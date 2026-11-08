// SPDX-License-Identifier: CC0-1.0

//! The three admission rules this suite pins, end to end over HTTP — which requests
//! CloudFront takes and which it refuses, and in which order it decides.
//!
//! * `MaxItems` is "the maximum number of items you want in the response", a BOUND on the
//!   page rather than a demand for one: a request that asks for none of them is served an
//!   empty page, reported as truncated while items remain, and only a count no page can
//!   have — a negative one — is refused with `InvalidArgument` (400). The rule holds for
//!   every listing of this service, so the rows below span listings whose page is a query
//!   parameter and one whose page is a member of the request document, resource kinds with
//!   items stored and with none, and page sizes at and around the bound.
//! * a distribution tenant is served by the multi-tenant distribution it is associated
//!   with, so a call that associates one with a distribution whose configuration is
//!   DISABLED is refused with `InvalidAssociation` (409) — decided from that distribution's
//!   own state BEFORE the tenant's arguments are measured against it, so the caller is told
//!   the condition it cannot fix by resubmitting, and nothing is stored.
//! * `UpdateDistribution` submits the distribution configuration WHOLE, so a document that
//!   leaves out a member the resource carries is refused with `IllegalUpdate` (400) rather
//!   than resolved: `PriceClass` "only supports standard distributions", so it is demanded
//!   of exactly the configurations that serve traffic directly.
//! * a `FunctionAssociation` names the event its function runs on, and a CloudFront function
//!   runs on a VIEWER event, so a configuration naming an origin-facing event type is
//!   refused — on the create and on the update alike — under the code this API answers a
//!   request document it cannot read with.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds, the tenant helpers
//! are [`crate::distribution_tenant_lifecycle`]'s and [`crate::distribution_tenants_list`]'s
//! and the tenant update is [`crate::distribution_tenant_update`]'s, so every assertion
//! travels the same path a client takes.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::{
    EVENT_TYPE_VALUES, PRICE_CLASS_VALUES,
};
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::DEFAULT_PRICE_CLASS;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    batch_xml, call, config, create_distribution, create_invalidation, emulator,
    update_distribution, Answer, API_VERSION,
};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, disabled_multi_tenant_distribution, managed_certificate_request,
    multi_tenant_distribution,
};
use crate::distribution_tenant_update::{update_tenant, update_xml};
use crate::distribution_tenants_list::{list_body, list_tenants, summaries, summary_ids};

// ---------------------------------------------------------------------------
// A page size of zero
// ---------------------------------------------------------------------------

/// One listing the page-size rule is asserted over: its URI, the element its page travels
/// in, and whether the fixture stored anything for it to list.
struct Listing {
    case: &'static str,
    /// The path the listing is bound to.
    path: String,
    /// The wrapper element the page travels in, e.g. `DistributionList`.
    wrapper: &'static str,
    /// Whether this listing has items to leave behind on an empty page.
    holds_items: bool,
    /// Whether the wrapper shape declares `IsTruncated` at all — `PublicKeyList` and
    /// `CachePolicyList` state the remainder with `NextMarker` alone
    /// (`smithy_query(query="shape",shape="CachePolicyList")`).
    states_truncation: bool,
}

/// `GET {path}{query}`.
async fn get_page(router: &MultiServiceRouter, path: &str, query: &str) -> Answer {
    call(router, "GET", &format!("{path}{query}"), None, None).await
}

/// Stand up one distribution with one invalidation, and answer the listings the rows below
/// run over.
async fn listings(router: &MultiServiceRouter) -> Vec<Listing> {
    let row = config("zero-page", "ref-zero-page");
    let created = create_distribution(router, &row.xml()).await;
    assert_eq!(
        created.status, 201,
        "case the fixture distribution: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let distribution_id = created
        .root()
        .text_at("Id")
        .expect("the created distribution reports an Id")
        .to_string();

    let invalidation = create_invalidation(
        router,
        &distribution_id,
        &batch_xml("zero-page-batch", &["/*"]),
    )
    .await;
    assert_eq!(
        invalidation.status, 201,
        "case the fixture invalidation: expected HTTP 201, got {} with {}",
        invalidation.status, invalidation.raw
    );

    vec![
        Listing {
            case: "the distributions of the account",
            path: format!("/{API_VERSION}/distribution"),
            wrapper: "DistributionList",
            holds_items: true,
            states_truncation: true,
        },
        Listing {
            case: "the invalidations of one distribution",
            path: format!("/{API_VERSION}/distribution/{distribution_id}/invalidation"),
            wrapper: "InvalidationList",
            holds_items: true,
            states_truncation: true,
        },
        // The AWS-managed cache policies are there without anything creating them, and
        // `CachePolicyList` declares no `IsTruncated`.
        Listing {
            case: "the cache policies of the account",
            path: format!("/{API_VERSION}/cache-policy"),
            wrapper: "CachePolicyList",
            holds_items: true,
            states_truncation: false,
        },
        // Nothing here creates one of these, so its listing has nothing to leave behind.
        Listing {
            case: "the streaming distributions of the account",
            path: format!("/{API_VERSION}/streaming-distribution"),
            wrapper: "StreamingDistributionList",
            holds_items: false,
            states_truncation: true,
        },
        Listing {
            case: "the origin access controls of the account",
            path: format!("/{API_VERSION}/origin-access-control"),
            wrapper: "OriginAccessControlList",
            holds_items: false,
            states_truncation: true,
        },
    ]
}

/// A `MaxItems` of zero bounds the page at no items, so every listing answers an empty page
/// — truncated exactly when items remain — and the sizes around it behave as the bound says:
/// one serves one item, and a negative count is no page at all.
#[tokio::test]
async fn a_page_size_of_zero_serves_an_empty_page() {
    let router = emulator();
    let rows = listings(&router).await;

    for row in &rows {
        let case = row.case;
        let empty = get_page(&router, &row.path, "?MaxItems=0").await;
        assert_eq!(
            empty.status, 200,
            "case {case} at MaxItems 0: expected HTTP 200, got {} with {}",
            empty.status, empty.raw
        );
        let page = empty.root();
        assert_eq!(
            page.name(),
            row.wrapper,
            "case {case} at MaxItems 0: expected the page in {}, got {}",
            row.wrapper,
            empty.raw
        );
        assert_eq!(
            page.text_at("MaxItems"),
            Some("0"),
            "case {case} at MaxItems 0: expected the size asked for echoed, got {}",
            empty.raw
        );
        assert_eq!(
            page.text_at("Quantity"),
            Some("0"),
            "case {case} at MaxItems 0: expected an empty page, got {}",
            empty.raw
        );
        assert!(
            page.child("Items").is_none(),
            "case {case} at MaxItems 0: expected no Items element, got {}",
            empty.raw
        );
        // `IsTruncated`, where the wrapper declares it, says whether items were left
        // behind, which is what the bound did to a listing that holds any.
        if row.states_truncation {
            assert_eq!(
                page.text_at("IsTruncated"),
                Some(if row.holds_items { "true" } else { "false" }),
                "case {case} at MaxItems 0: expected IsTruncated {}, got {}",
                row.holds_items,
                empty.raw
            );
        }
        // A page that reports nothing names no resume point.
        assert!(
            page.child("NextMarker").is_none(),
            "case {case} at MaxItems 0: expected no NextMarker, got {}",
            empty.raw
        );

        // The smallest page that can carry an item carries one exactly where there is one.
        let one = get_page(&router, &row.path, "?MaxItems=1").await;
        assert_eq!(
            one.status, 200,
            "case {case} at MaxItems 1: expected HTTP 200, got {} with {}",
            one.status, one.raw
        );
        assert_eq!(
            one.root().text_at("Quantity"),
            Some(if row.holds_items { "1" } else { "0" }),
            "case {case} at MaxItems 1: expected the one-item page, got {}",
            one.raw
        );

        // A count no page can have is refused, and the refusal states the constraint.
        for size in ["-1", "-2147483648"] {
            let refused = get_page(&router, &row.path, &format!("?MaxItems={size}")).await;
            assert_eq!(
                refused.status, 400,
                "case {case} at MaxItems {size}: expected HTTP 400, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                "InvalidArgument",
                "case {case} at MaxItems {size}: expected InvalidArgument, got {}",
                refused.raw
            );
        }
    }
}

/// The same rule on a listing whose page size travels in the request DOCUMENT rather than in
/// the query: `ListDistributionTenants` serves the empty page a zero asks for and refuses a
/// negative count.
#[tokio::test]
async fn a_page_size_of_zero_serves_an_empty_page_in_a_document_listing() {
    let router = emulator();
    let distribution = multi_tenant_distribution(&router, "ref-zero-page-tenants").await;
    let created = create_distribution_tenant(
        &router,
        &tenant_xml(&distribution, "zero-page-tenant", Some("zero-page-tenant")),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the fixture tenant: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );

    let filter = Some((Some(distribution.as_str()), None));
    let empty = list_tenants(&router, &list_body(filter, None, Some(0))).await;
    assert_eq!(
        empty.status, 200,
        "case the tenants at MaxItems 0: expected HTTP 200, got {} with {}",
        empty.status, empty.raw
    );
    assert!(
        summaries(&empty).is_empty(),
        "case the tenants at MaxItems 0: expected an empty page, got {}",
        empty.raw
    );

    let one = list_tenants(&router, &list_body(filter, None, Some(1))).await;
    assert_eq!(
        summaries(&one).len(),
        1,
        "case the tenants at MaxItems 1: expected the one stored tenant, got {}",
        one.raw
    );

    let refused = list_tenants(&router, &list_body(filter, None, Some(-1))).await;
    assert_eq!(
        refused.status, 400,
        "case the tenants at MaxItems -1: expected HTTP 400, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "InvalidArgument",
        "case the tenants at MaxItems -1: expected InvalidArgument, got {}",
        refused.raw
    );
}

// ---------------------------------------------------------------------------
// A tenant under a disabled multi-tenant distribution
// ---------------------------------------------------------------------------

/// The sentence CloudFront refuses an association with a disabled multi-tenant distribution
/// with.
const DISABLED_DISTRIBUTION_MESSAGE: &str = "The specified multi-tenant distribution is disabled.";

/// A `CreateDistributionTenantRequest` for `name` under `distribution_id`, stating the
/// required parameter value when `parameter` names one.
fn tenant_xml(distribution_id: &str, name: &str, parameter: Option<&str>) -> String {
    let parameters = match parameter {
        Some(value) => format!(
            "<Parameters><member><Name>tenantName</Name><Value>{value}</Value></member>\
             </Parameters>"
        ),
        None => String::new(),
    };
    format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{distribution_id}</DistributionId><Name>{name}</Name>\
         <Domains><member><Domain>{name}.example.com</Domain></member></Domains>\
         {parameters}{}</CreateDistributionTenantRequest>",
        managed_certificate_request()
    )
}

/// Every create against a DISABLED multi-tenant distribution is refused with the one
/// conflict that condition is — whatever the tenant's own arguments are, so the refusal is
/// never the arguments' — and the account holds no tenant afterwards.
#[tokio::test]
async fn a_tenant_is_not_created_under_a_disabled_multi_tenant_distribution() {
    let router = emulator();
    let disabled = disabled_multi_tenant_distribution(&router, "ref-disabled-multi-tenant").await;

    // The rows differ along the tenant arguments the create carries: a complete set, a
    // request that states none of the required parameters (which the distribution's own
    // parameter definitions would refuse), and a name the account already holds nothing
    // under. The distribution's state is decided first, so every row reads the same.
    for (case, body) in [
        (
            "a create naming every required parameter",
            tenant_xml(&disabled, "disabled-complete", Some("disabled-complete")),
        ),
        (
            "a create naming no parameter the distribution requires",
            tenant_xml(&disabled, "disabled-no-parameters", None),
        ),
        (
            "a create whose domain and name are free",
            tenant_xml(&disabled, "disabled-free-names", Some("disabled-free-names")),
        ),
    ] {
        let refused = create_distribution_tenant(&router, &body).await;
        assert_eq!(
            refused.status, 409,
            "case {case}: expected HTTP 409, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidAssociation",
            "case {case}: expected InvalidAssociation, got {}",
            refused.raw
        );
        assert_eq!(
            refused.root().find_text("Message"),
            Some(DISABLED_DISTRIBUTION_MESSAGE),
            "case {case}: expected the condition's own sentence, got {}",
            refused.raw
        );
    }

    // The refusals stored nothing: the listing of that distribution's tenants is empty.
    let listed = list_tenants(
        &router,
        &list_body(Some((Some(disabled.as_str()), None)), None, None),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case the tenants of the disabled distribution: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    assert!(
        summaries(&listed).is_empty(),
        "case the tenants of the disabled distribution: expected an empty list, got {}",
        listed.raw
    );

    // The control: the SAME create under an enabled multi-tenant distribution is served, and
    // the tenant it stored is read back by both readers.
    let enabled = multi_tenant_distribution(&router, "ref-enabled-multi-tenant").await;
    let created = create_distribution_tenant(
        &router,
        &tenant_xml(&enabled, "enabled-complete", Some("enabled-complete")),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case a create under an enabled distribution: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let tenant_id = created
        .root()
        .text_at("Id")
        .expect("the created tenant reports an Id")
        .to_string();
    let read = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution-tenant/{tenant_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case a create under an enabled distribution: expected the read to succeed, got {} with {}",
        read.status, read.raw
    );
    assert_eq!(
        read.root().text_at("DistributionId"),
        Some(enabled.as_str()),
        "case a create under an enabled distribution: expected the read to name the \
         distribution, got {}",
        read.raw
    );
    let listed = list_tenants(
        &router,
        &list_body(Some((Some(enabled.as_str()), None)), None, None),
    )
    .await;
    assert_eq!(
        summary_ids(&listed),
        vec![tenant_id.clone()],
        "case a create under an enabled distribution: expected the listing to report it, got {}",
        listed.raw
    );

    // The same rule on the call that MOVES a tenant: an update that associates this tenant
    // with the disabled distribution is the same conflict, and the tenant stays where it is.
    let etag = created.header("etag").to_string();
    let moved = update_tenant(
        &router,
        &tenant_id,
        &update_xml(&format!(
            "<DistributionId>{disabled}</DistributionId>"
        )),
        Some(&etag),
    )
    .await;
    assert_eq!(
        moved.status, 409,
        "case an update onto the disabled distribution: expected HTTP 409, got {} with {}",
        moved.status, moved.raw
    );
    assert_eq!(
        moved.error_code(),
        "InvalidAssociation",
        "case an update onto the disabled distribution: expected InvalidAssociation, got {}",
        moved.raw
    );
    assert_eq!(
        moved.root().find_text("Message"),
        Some(DISABLED_DISTRIBUTION_MESSAGE),
        "case an update onto the disabled distribution: expected the condition's own sentence, \
         got {}",
        moved.raw
    );
    let reread = call(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution-tenant/{tenant_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(
        reread.root().text_at("DistributionId"),
        Some(enabled.as_str()),
        "case an update onto the disabled distribution: expected the tenant to stay where it \
         was, got {}",
        reread.raw
    );
}

// ---------------------------------------------------------------------------
// An update states the whole configuration
// ---------------------------------------------------------------------------

/// The sentence CloudFront refuses an update whose configuration states no price class with.
const PRICE_CLASS_MISSING_MESSAGE: &str = "Price class is missing for the resource";

/// A `DistributionConfig` for `reference` stating `price_class` when it names one, and
/// `extra` at the configuration level.
fn update_config_xml(reference: &str, price_class: Option<&str>, extra: &str) -> String {
    let stated = match price_class {
        Some(value) => format!("<PriceClass>{value}</PriceClass>"),
        None => String::new(),
    };
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy></DefaultCacheBehavior>\
         <Comment>{reference}</Comment>{stated}<Enabled>true</Enabled>{extra}\
         </DistributionConfig>"
    )
}

/// `GET /2020-05-31/distribution/{Id}/config`.
async fn get_config(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        None,
        None,
    )
    .await
}

/// An update whose configuration states no price class is refused and changes nothing, while
/// one stating any class the model admits lands and is read back — and a multi-tenant
/// configuration, which the member does not support, is complete without it.
#[tokio::test]
async fn an_update_states_the_whole_configuration() {
    let router = emulator();
    let reference = "ref-whole-config";
    let created = create_distribution(
        &router,
        &update_config_xml(reference, Some(DEFAULT_PRICE_CLASS), ""),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the fixture distribution: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .expect("the created distribution reports an Id")
        .to_string();
    let mut etag = created.header("etag").to_string();

    // The partial document is refused, and the version it did not land on is still current.
    let refused = update_distribution(
        &router,
        &id,
        &update_config_xml(reference, None, ""),
        Some(&etag),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case an update stating no price class: expected HTTP 400, got {} with {}",
        refused.status, refused.raw
    );
    assert_eq!(
        refused.error_code(),
        "IllegalUpdate",
        "case an update stating no price class: expected IllegalUpdate, got {}",
        refused.raw
    );
    assert_eq!(
        refused.root().find_text("Message"),
        Some(PRICE_CLASS_MISSING_MESSAGE),
        "case an update stating no price class: expected the member's own sentence, got {}",
        refused.raw
    );
    let unchanged = get_config(&router, &id).await;
    assert_eq!(
        unchanged.header("etag"),
        etag,
        "case an update stating no price class: expected the stored version to survive, got {}",
        unchanged.raw
    );
    assert_eq!(
        unchanged.root().text_at("PriceClass"),
        Some(DEFAULT_PRICE_CLASS),
        "case an update stating no price class: expected the stored class to survive, got {}",
        unchanged.raw
    );

    // Every class the model admits is a complete configuration
    // (`smithy_query(query="shape",shape="PriceClass")` -> PRICE_CLASS_VALUES), and the one
    // submitted is what the config read reports back.
    for price_class in PRICE_CLASS_VALUES {
        let updated = update_distribution(
            &router,
            &id,
            &update_config_xml(reference, Some(price_class), ""),
            Some(&etag),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case an update stating {price_class}: expected HTTP 200, got {} with {}",
            updated.status, updated.raw
        );
        etag = updated.header("etag").to_string();
        let read = get_config(&router, &id).await;
        assert_eq!(
            read.root().text_at("PriceClass"),
            Some(*price_class),
            "case an update stating {price_class}: expected the class read back, got {}",
            read.raw
        );
    }

    // A MULTI-TENANT distribution's configuration cannot state the member at all, so it is
    // complete without it: the same update that is refused above is served here.
    let tenant_config = "<ConnectionMode>tenant-only</ConnectionMode>\
         <TenantConfig><ParameterDefinitions>\
         <member><Name>tenantName</Name><Definition><StringSchema>\
         <Required>true</Required></StringSchema></Definition></member>\
         </ParameterDefinitions></TenantConfig>";
    let multi_reference = "ref-whole-config-tenant";
    let multi = create_distribution(
        &router,
        &update_config_xml(multi_reference, None, tenant_config),
    )
    .await;
    assert_eq!(
        multi.status, 201,
        "case the multi-tenant distribution: expected HTTP 201, got {} with {}",
        multi.status, multi.raw
    );
    let multi_id = multi
        .root()
        .text_at("Id")
        .expect("the created distribution reports an Id")
        .to_string();
    let multi_etag = multi.header("etag").to_string();
    let updated = update_distribution(
        &router,
        &multi_id,
        &update_config_xml(multi_reference, None, tenant_config),
        Some(&multi_etag),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case a multi-tenant update stating no price class: expected HTTP 200, got {} with {}",
        updated.status, updated.raw
    );
}

// ---------------------------------------------------------------------------
// A function association names a viewer event
// ---------------------------------------------------------------------------

/// The `FunctionAssociations` wrapper attaching `arn` at `event_type`, stating its own
/// length.
fn function_associations(arn: &str, event_type: &str) -> String {
    format!(
        "<FunctionAssociations><Quantity>1</Quantity><Items><FunctionAssociation>\
         <FunctionARN>{arn}</FunctionARN><EventType>{event_type}</EventType>\
         </FunctionAssociation></Items></FunctionAssociations>"
    )
}

/// A `DistributionConfig` whose DEFAULT cache behaviour carries `associations`.
fn config_with_associations(reference: &str, associations: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{associations}\
         </DefaultCacheBehavior>\
         <Comment>{reference}</Comment>\
         <PriceClass>{DEFAULT_PRICE_CLASS}</PriceClass><Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// A `FunctionAssociation` names the event the function runs on, and a CloudFront function
/// runs on a VIEWER event: `EventType` admits `viewer-request` and `viewer-response`, and
/// "you cannot use origin-facing event types (origin-request and origin-response) with a
/// CloudFront function"
/// (`smithy_query(query="doc",shape="FunctionAssociation",member="EventType")`). Every value
/// of the model's `EventType` enum is a row: the viewer half is stored, and the
/// origin-facing half is refused on the create and on the update alike.
#[tokio::test]
async fn a_function_association_names_a_viewer_event() {
    let router = emulator();

    // The function the associations name: published, so nothing but the event type is
    // under test.
    let created_function = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/function"),
        Some(&format!(
            "<CreateFunctionRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>admission-event-fn</Name>\
             <FunctionConfig><Comment>admission</Comment>\
             <Runtime>cloudfront-js-2.0</Runtime></FunctionConfig>\
             <FunctionCode>ZnVuY3Rpb24gaGFuZGxlcihldmVudCkgeyByZXR1cm4gZXZlbnQucmVxdWVzdDsgfQ==\
             </FunctionCode></CreateFunctionRequest>"
        )),
        None,
    )
    .await;
    assert_eq!(
        created_function.status, 201,
        "case the fixture function: expected HTTP 201, got {} with {}",
        created_function.status, created_function.raw
    );
    let function_arn = created_function
        .root()
        .find_text("FunctionARN")
        .expect("the created function reports an ARN")
        .to_string();
    let function_etag = created_function.header("etag").to_string();
    let published = call(
        &router,
        "POST",
        &format!("/{API_VERSION}/function/admission-event-fn/publish"),
        None,
        Some(&function_etag),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "case the fixture function: expected the publish to succeed, got {} with {}",
        published.status, published.raw
    );

    // A distribution with no association at all, which every update below rewrites.
    let created = create_distribution(
        &router,
        &config_with_associations("ref-association-events", ""),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case the fixture distribution: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .expect("the created distribution reports an Id")
        .to_string();
    let mut etag = created.header("etag").to_string();

    // One row per value the model's `EventType` enum admits
    // (`smithy_query(query="shape",shape="EventType")` -> EVENT_TYPE_VALUES), each with
    // whether a CloudFront function runs on it.
    for event_type in EVENT_TYPE_VALUES {
        let viewer_event = event_type.starts_with("viewer-");
        let body = config_with_associations(
            "ref-association-events",
            &function_associations(&function_arn, event_type),
        );
        let updated = update_distribution(&router, &id, &body, Some(&etag)).await;
        if viewer_event {
            assert_eq!(
                updated.status, 200,
                "case an update at {event_type}: expected HTTP 200, got {} with {}",
                updated.status, updated.raw
            );
            etag = updated.header("etag").to_string();
            let read = get_config(&router, &id).await;
            assert_eq!(
                read.root()
                    .descendant("FunctionAssociation")
                    .and_then(|association| association.text_at("EventType")),
                Some(*event_type),
                "case an update at {event_type}: expected the association read back, got {}",
                read.raw
            );
            continue;
        }
        assert_eq!(
            updated.status, 400,
            "case an update at {event_type}: expected HTTP 400, got {} with {}",
            updated.status, updated.raw
        );
        assert_eq!(
            updated.error_code(),
            "MalformedXML",
            "case an update at {event_type}: expected MalformedXML, got {}",
            updated.raw
        );
        let message = updated.root().find_text("Message").unwrap_or_default();
        assert!(
            message.contains(&format!("Value '{event_type}' at")),
            "case an update at {event_type}: expected the refused value stated, got {message:?}"
        );
        assert!(
            message.contains(
                "'distributionConfig.defaultCacheBehavior.functionAssociations.items.1.member.\
                 eventType'"
            ),
            "case an update at {event_type}: expected the member path stated, got {message:?}"
        );
        assert!(
            message.ends_with("Member must satisfy enum value set: [viewer-request, viewer-response]"),
            "case an update at {event_type}: expected the admitted set stated, got {message:?}"
        );

        // The refusal stored nothing: the stored configuration still carries the
        // association the last accepted update left, under the same version.
        let unchanged = get_config(&router, &id).await;
        assert_eq!(
            unchanged.header("etag"),
            etag,
            "case an update at {event_type}: expected the stored version to survive, got {}",
            unchanged.raw
        );

        // The same configuration is refused on the way IN too, so no create can store one
        // either.
        let refused_create = create_distribution(
            &router,
            &config_with_associations(
                &format!("ref-association-{event_type}"),
                &function_associations(&function_arn, event_type),
            ),
        )
        .await;
        assert_eq!(
            refused_create.status, 400,
            "case a create at {event_type}: expected HTTP 400, got {} with {}",
            refused_create.status, refused_create.raw
        );
        assert_eq!(
            refused_create.error_code(),
            "MalformedXML",
            "case a create at {event_type}: expected MalformedXML, got {}",
            refused_create.raw
        );
    }
}
