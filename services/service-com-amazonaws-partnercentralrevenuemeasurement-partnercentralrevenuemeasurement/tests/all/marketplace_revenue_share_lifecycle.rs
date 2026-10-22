// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the marketplace revenue share slice, driven over this
//! service's own `rpcv2Cbor` wire protocol through the same `MultiServiceRouter`
//! the production emulator dispatches with.
//!
//! Every test asserts a RULE over a SET of inputs: the shapes the bootstrap issue
//! names are one row each.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use service_com_amazonaws_partnercentralrevenuemeasurement_partnercentralrevenuemeasurement as prm;
use smithy_server_runtime::MultiServiceRouter;

use crate::cbor_request::{request_body, texts, Cbor};
use prm::ops::types::{
    CreateMarketplaceRevenueShareAllocationInput, CreateMarketplaceRevenueShareAllocationOutput,
    CreateMarketplaceRevenueShareInput, CreateMarketplaceRevenueShareOutput,
    GetMarketplaceRevenueShareAllocationInput, GetMarketplaceRevenueShareAllocationOutput,
    GetMarketplaceRevenueShareInput, GetMarketplaceRevenueShareOutput,
    ListMarketplaceRevenueShareAllocationsInput, ListMarketplaceRevenueShareAllocationsOutput,
    ListMarketplaceRevenueSharesInput, ListMarketplaceRevenueSharesOutput, Tag,
    UpdateMarketplaceRevenueShareAllocationInput, UpdateMarketplaceRevenueShareAllocationOutput,
    CREATE_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
    CREATE_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
    GET_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
    GET_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
    LIST_MARKETPLACE_REVENUE_SHARES_INPUT_SCHEMA,
    LIST_MARKETPLACE_REVENUE_SHARE_ALLOCATIONS_INPUT_SCHEMA,
    UPDATE_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
};

/// The Region the request's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// This service's SigV4 signing name, which is also the router's route key.
const SIGNING_NAME: &str = "partnercentral";

/// The canonical emulator account id every ARN carries.
const ACCOUNT_ID: &str = "000000000000";

/// The Smithy service shape name, which is the `rpcv2Cbor` URI's service
/// segment.
const SERVICE_SHAPE: &str = "PartnerCentralRevenueMeasurement";

/// A first-of-month date comfortably in the future, so an allocation created
/// with it is "future-dated" for as long as this emulator exists and the
/// model's time-based mutability rules allow it to be updated.
const FUTURE_FROM: &str = "2099-01-01";

/// A last-of-month date after [`FUTURE_FROM`].
const FUTURE_UNTIL: &str = "2099-12-31";

/// A first-of-month date in the past, so the allocation it starts is already in
/// force and the mutability rules refuse the members they protect.
const PAST_FROM: &str = "2020-01-01";

/// One emulator holding exactly this service, dispatched over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}

/// A dispatched response: the HTTP status and the raw CBOR body.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    /// The decoded output of a call that must have succeeded.
    fn ok<T: DeserializableStruct>(&self, case: &str) -> T {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            self.status,
            self.render()
        );
        let codec = CborCodec::default();
        let mut deserializer = codec.create_deserializer(&self.body);
        T::deserialize(&mut deserializer)
            .unwrap_or_else(|error| panic!("case {case:?}: undecodable response body: {error}"))
    }

    /// Assert a modelled failure: the status the model gives the error, and the
    /// error code the `rpcv2Cbor` envelope carries in `__type`.
    fn is_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            self.status,
            status,
            "case {case:?}: expected HTTP {status} {code}, got {} with body {}",
            self.status,
            self.render()
        );
        assert!(
            self.mentions(code),
            "case {case:?}: expected the error code {code:?} in the body, got {}",
            self.render()
        );
    }

    /// Whether the CBOR body carries `needle` as a text string. CBOR writes a
    /// definite-length text string as its UTF-8 bytes after a header byte, so a
    /// byte-window search finds any string member's value.
    fn mentions(&self, needle: &str) -> bool {
        self.body
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    /// The body as lossy text, for an assertion message.
    fn render(&self) -> String {
        String::from_utf8_lossy(&self.body).escape_debug().to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        // The SINGLE shared engine, exactly as `router/src/lib.rs` builds it.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(prm::EmulatedService::new(engine));
        Self { router }
    }

    async fn call<I: SerializableStruct>(
        &self,
        operation: &str,
        schema: &Schema,
        input: &I,
    ) -> Answer {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(schema, input)
            .expect("a serializable request body");
        self.call_raw(operation, serializer.finish()).await
    }

    async fn call_raw(&self, operation: &str, payload: Vec<u8>) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/service/{SERVICE_SHAPE}/operation/{operation}"))
            .header("authorization", authorization)
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.body().to_vec(),
        }
    }

    async fn create_share(&self, input: &CreateMarketplaceRevenueShareInput) -> Answer {
        self.call(
            "CreateMarketplaceRevenueShare",
            CREATE_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn get_share(&self, input: &GetMarketplaceRevenueShareInput) -> Answer {
        self.call(
            "GetMarketplaceRevenueShare",
            GET_MARKETPLACE_REVENUE_SHARE_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn list_shares(&self, input: &ListMarketplaceRevenueSharesInput) -> Answer {
        self.call(
            "ListMarketplaceRevenueShares",
            LIST_MARKETPLACE_REVENUE_SHARES_INPUT_SCHEMA,
            input,
        )
        .await
    }

    /// `ListMarketplaceRevenueShares` with a body built directly, which is the
    /// only way to send one of its list-of-scalar filters. See
    /// `crate::cbor_request`.
    async fn list_shares_raw(&self, entries: Vec<(&str, Cbor)>) -> Answer {
        self.call_raw("ListMarketplaceRevenueShares", request_body(entries))
            .await
    }

    async fn create_allocation(
        &self,
        input: &CreateMarketplaceRevenueShareAllocationInput,
    ) -> Answer {
        self.call(
            "CreateMarketplaceRevenueShareAllocation",
            CREATE_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn get_allocation(&self, input: &GetMarketplaceRevenueShareAllocationInput) -> Answer {
        self.call(
            "GetMarketplaceRevenueShareAllocation",
            GET_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn list_allocations(
        &self,
        input: &ListMarketplaceRevenueShareAllocationsInput,
    ) -> Answer {
        self.call(
            "ListMarketplaceRevenueShareAllocations",
            LIST_MARKETPLACE_REVENUE_SHARE_ALLOCATIONS_INPUT_SCHEMA,
            input,
        )
        .await
    }

    async fn update_allocation(
        &self,
        input: &UpdateMarketplaceRevenueShareAllocationInput,
    ) -> Answer {
        self.call(
            "UpdateMarketplaceRevenueShareAllocation",
            UPDATE_MARKETPLACE_REVENUE_SHARE_ALLOCATION_INPUT_SCHEMA,
            input,
        )
        .await
    }
}

/// A `MarketplaceProductId`: `^prod-[a-z0-9]{13}$`, 18 characters.
fn product_id(suffix: &str) -> String {
    let id = format!("prod-{suffix}");
    assert_eq!(id.len(), 18, "a product id is 18 characters: {id:?}");
    id
}

/// `MarketplaceRevenueShareAllocationId`'s pattern, `^mrsa-[A-Za-z0-9]{13}$`, as
/// a predicate.
fn is_allocation_id(value: &str) -> bool {
    value.len() == 18
        && value.starts_with("mrsa-")
        && value[5..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// The ARN the model's `ResourceARN` pattern spells for a revenue share.
fn expected_share_arn(catalog: &str, product: &str) -> String {
    format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT_ID}:catalog/{catalog}/marketplace-revenue-share/{product}"
    )
}

/// A create request for `catalog`/`product`, with no tags and no client token.
fn create_share_input(catalog: &str, product: &str) -> CreateMarketplaceRevenueShareInput {
    CreateMarketplaceRevenueShareInput {
        catalog: catalog.to_string(),
        product_id: product.to_string(),
        ..Default::default()
    }
}

/// A create-allocation request with only the required members set.
fn create_allocation_input(
    catalog: &str,
    product: &str,
    effective_from: &str,
    percent: &str,
) -> CreateMarketplaceRevenueShareAllocationInput {
    CreateMarketplaceRevenueShareAllocationInput {
        catalog: catalog.to_string(),
        product_id: product.to_string(),
        effective_from: effective_from.to_string(),
        revenue_share_percent: percent.to_string(),
        ..Default::default()
    }
}

/// Month ends across the Gregorian leap rule, which the `time` crate answers
/// for `state::parse_date` / `state::is_last_day_of_month`: these rows are what
/// pin that the service asks it the right question on each side of a month
/// boundary. Each row is `(case, EffectiveFrom, EffectiveUntil)` where
/// the end date IS the last day of its month, so the create must be accepted:
/// a leap February, a common February, a century year divisible by 400 (a leap
/// year), a century year that is not, a 30- and a 31-day month, and the last
/// month RFC 3339 represents — whose end date is the one whose SUCCESSOR the
/// date type cannot render, which the service must answer rather than unwind
/// on (`state::is_last_day_of_month`).
fn month_end_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("a leap February", "2024-02-01", "2024-02-29"),
        ("a common February", "2023-02-01", "2023-02-28"),
        ("a century year divisible by 400", "2000-02-01", "2000-02-29"),
        ("a century year not divisible by 400", "2100-02-01", "2100-02-28"),
        ("a thirty-day month", "2024-04-01", "2024-04-30"),
        ("a thirty-one-day month", "2024-12-01", "2024-12-31"),
        (
            "the last month RFC 3339 represents",
            "9999-12-01",
            "9999-12-31",
        ),
    ]
}

/// The same rule's refusals: `(case, EffectiveFrom, EffectiveUntil, member)`,
/// each value satisfying `^\d{4}-\d{2}-\d{2}$` so the refusal is this service's
/// and not the boundary validator's.
fn month_end_refusals() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "the 29th of a February in a century year not divisible by 400",
            "2100-02-01",
            "2100-02-29",
            "EffectiveUntil",
        ),
        (
            "the 29th of February 1900, which was not a leap year",
            "1900-02-01",
            "1900-02-29",
            "EffectiveUntil",
        ),
        (
            "the 31st of a thirty-day month",
            "2024-04-01",
            "2024-04-31",
            "EffectiveUntil",
        ),
        (
            "the 28th of a leap February, which is not its last day",
            "2024-02-01",
            "2024-02-28",
            "EffectiveUntil",
        ),
        (
            "month zero",
            "2024-01-01",
            "2024-00-31",
            "EffectiveUntil",
        ),
        ("day zero", "2024-01-00", "2024-01-31", "EffectiveFrom"),
        // One past the published lower bound of the date range: `^\d{4}-\d{2}-\d{2}$`
        // admits year 0000, which is outside the years RFC 3339 represents
        // (0001..=9999), so it earns the modelled 400 rather than a panic in
        // the formatter.
        (
            "a year below the first RFC 3339 represents",
            "0000-01-01",
            "0000-01-31",
            "EffectiveFrom",
        ),
    ]
}

#[tokio::test]
async fn the_month_boundary_rule_follows_the_gregorian_leap_rule() {
    let emulator = Emulator::new();
    let catalog = "AWS";
    let product = product_id("abc1234567890");
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&create_share_input(catalog, &product))
        .await
        .ok("the parent share");

    for (case, from, until) in month_end_rows() {
        let created: CreateMarketplaceRevenueShareAllocationOutput = emulator
            .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
                effective_until: Some(until.to_string()),
                ..create_allocation_input(catalog, &product, from, "10")
            })
            .await
            .ok(case);
        assert_eq!(
            (
                created.effective_from.as_str(),
                created.effective_until.as_deref()
            ),
            (from, Some(until)),
            "case {case:?}: expected the stored interval back, got {created:?}"
        );
    }

    for (case, from, until, member) in month_end_refusals() {
        let answer = emulator
            .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
                effective_until: Some(until.to_string()),
                ..create_allocation_input(catalog, &product, from, "10")
            })
            .await;
        answer.is_error(case, 400, "ValidationException");
        assert!(
            answer.mentions(member),
            "case {case:?}: expected the refusal to name {member:?}, got {}",
            answer.render()
        );
    }
}

// ---------------------------------------------------------------------------
// Step 1 + 2 of the slice: create a share, read it back.
// ---------------------------------------------------------------------------

/// One row of the create-and-read-back table.
struct ShareRow {
    case: &'static str,
    catalog: &'static str,
    suffix: &'static str,
    tags: Option<Vec<Tag>>,
}

fn share_rows() -> Vec<ShareRow> {
    vec![
        ShareRow {
            case: "the AWS catalog, the shape the issue records",
            catalog: "AWS",
            suffix: "abc1234567890",
            tags: None,
        },
        ShareRow {
            case: "the Sandbox catalog, which is a separate namespace",
            catalog: "Sandbox",
            suffix: "abc1234567890",
            tags: None,
        },
        ShareRow {
            case: "a product id of digits only",
            catalog: "AWS",
            suffix: "0000000000001",
            tags: Some(vec![Tag {
                key: "owner".to_string(),
                value: "revenue-team".to_string(),
            }]),
        },
        ShareRow {
            case: "a product id of letters only, with two tags",
            catalog: "Sandbox",
            suffix: "zzzzzzzzzzzzz",
            tags: Some(vec![
                Tag {
                    key: "owner".to_string(),
                    value: "revenue-team".to_string(),
                },
                Tag {
                    key: "cost-centre".to_string(),
                    value: "4711".to_string(),
                },
            ]),
        },
        ShareRow {
            case: "a product id mixing both",
            catalog: "AWS",
            suffix: "a1b2c3d4e5f60",
            tags: Some(vec![]),
        },
    ]
}

#[tokio::test]
async fn a_created_revenue_share_is_read_back_member_for_member() {
    let emulator = Emulator::new();
    let mut seen_arns: BTreeSet<String> = BTreeSet::new();

    for row in share_rows() {
        let product = product_id(row.suffix);
        let created: CreateMarketplaceRevenueShareOutput = emulator
            .create_share(&CreateMarketplaceRevenueShareInput {
                tags: row.tags.clone(),
                ..create_share_input(row.catalog, &product)
            })
            .await
            .ok(row.case);

        assert_eq!(
            created.product_id, product,
            "case {:?}: expected the product id back, got {:?}",
            row.case, created.product_id
        );
        assert_eq!(
            created.arn,
            expected_share_arn(row.catalog, &product),
            "case {:?}: expected the catalog-scoped ARN, got {:?}",
            row.case,
            created.arn
        );
        assert!(
            seen_arns.insert(created.arn.clone()),
            "case {:?}: expected a fresh ARN, got the already-minted {:?}",
            row.case,
            created.arn
        );
        assert_eq!(
            created.catalog.as_deref(),
            Some(row.catalog),
            "case {:?}: expected the catalog back, got {:?}",
            row.case,
            created.catalog
        );
        assert_eq!(
            created.revision,
            Some(1),
            "case {:?}: expected a fresh share at revision 1, got {:?}",
            row.case,
            created.revision
        );
        assert_eq!(
            created.product_code.as_deref(),
            Some(row.suffix),
            "case {:?}: expected the product code derived from the product id, got {:?}",
            row.case,
            created.product_code
        );
        assert_eq!(
            created.created_date, created.last_modified_date,
            "case {:?}: expected a fresh share's two dates to agree, got {:?} and {:?}",
            row.case, created.created_date, created.last_modified_date
        );

        // Step 2: the read is answered from the same stored record.
        let fetched: GetMarketplaceRevenueShareOutput = emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: row.catalog.to_string(),
                product_id: product.clone(),
                revision: None,
            })
            .await
            .ok(row.case);
        assert_eq!(
            (
                fetched.arn.as_str(),
                fetched.catalog.as_str(),
                fetched.product_code.as_deref(),
                fetched.created_date,
                fetched.revision,
            ),
            (
                created.arn.as_str(),
                row.catalog,
                created.product_code.as_deref(),
                created.created_date,
                created.revision,
            ),
            "case {:?}: expected the get to answer the created record, got {fetched:?}",
            row.case
        );
        // `ProductName` is the AWS Marketplace display name, which this emulator
        // does not hold; the member is optional, so it is left absent rather
        // than answered with generated text.
        assert_eq!(
            (
                created.product_name.as_deref(),
                fetched.product_name.as_deref()
            ),
            (None, None),
            "case {:?}: expected ProductName to be absent, got {:?} and {:?}",
            row.case,
            created.product_name,
            fetched.product_name
        );
        assert_eq!(
            (
                fetched.latest_revision,
                fetched.total_marketplace_revenue_share_allocation_count,
                fetched.total_active_marketplace_revenue_share_allocation_count,
            ),
            (Some(1), Some(0), Some(0)),
            "case {:?}: expected a share with no allocations to count zero, got {fetched:?}",
            row.case
        );
    }

    // A share in one catalog is invisible in the other, which is what makes the
    // catalog part of the resource's identity.
    let only_in_sandbox = product_id("zzzzzzzzzzzzz");
    emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: "AWS".to_string(),
            product_id: only_in_sandbox.clone(),
            revision: None,
        })
        .await
        .is_error(
            "a Sandbox-only product read from the AWS catalog",
            404,
            "ResourceNotFoundException",
        );
}

// ---------------------------------------------------------------------------
// Step 3 + 4 of the slice: create an allocation, read it back.
// ---------------------------------------------------------------------------

/// One row of the allocation round-trip table.
struct AllocationRow {
    case: &'static str,
    effective_from: &'static str,
    effective_until: Option<&'static str>,
    percent: &'static str,
}

fn allocation_rows() -> Vec<AllocationRow> {
    vec![
        AllocationRow {
            case: "the shape the issue records: open-ended, one decimal place",
            effective_from: "2024-01-01",
            effective_until: None,
            percent: "12.5",
        },
        AllocationRow {
            case: "a bounded interval inside one month",
            effective_from: "2024-03-01",
            effective_until: Some("2024-03-31"),
            percent: "7.25",
        },
        AllocationRow {
            // February 2024 has 29 days, so its last day is a leap-year value.
            case: "a leap February, whose last day is the 29th",
            effective_from: "2024-02-01",
            effective_until: Some("2024-02-29"),
            percent: "0",
        },
        AllocationRow {
            // February 2023 has 28 days.
            case: "a common February, whose last day is the 28th",
            effective_from: "2023-02-01",
            effective_until: Some("2023-02-28"),
            percent: "100",
        },
        AllocationRow {
            case: "a thirty-day month, and the largest percentage with two decimals",
            effective_from: "2025-04-01",
            effective_until: Some("2025-04-30"),
            percent: "99.99",
        },
        AllocationRow {
            case: "an interval spanning years",
            effective_from: "2022-11-01",
            effective_until: Some("2026-01-31"),
            percent: "33.33",
        },
    ]
}

#[tokio::test]
async fn an_allocation_round_trips_and_moves_the_parent_share_revision() {
    let emulator = Emulator::new();
    let catalog = "AWS";
    let product = product_id("abc1234567890");
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&create_share_input(catalog, &product))
        .await
        .ok("the parent share");

    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let rows = allocation_rows();
    for (index, row) in rows.iter().enumerate() {
        let created: CreateMarketplaceRevenueShareAllocationOutput = emulator
            .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
                effective_until: row.effective_until.map(str::to_string),
                ..create_allocation_input(catalog, &product, row.effective_from, row.percent)
            })
            .await
            .ok(row.case);

        assert!(
            is_allocation_id(&created.marketplace_revenue_share_allocation_id),
            "case {:?}: expected an id matching ^mrsa-[A-Za-z0-9]{{13}}$, got {:?}",
            row.case,
            created.marketplace_revenue_share_allocation_id
        );
        assert!(
            seen_ids.insert(created.marketplace_revenue_share_allocation_id.clone()),
            "case {:?}: expected a fresh allocation id, got the already-minted {:?}",
            row.case,
            created.marketplace_revenue_share_allocation_id
        );
        assert_eq!(
            created.status, "ACTIVE",
            "case {:?}: expected a newly created allocation to be ACTIVE, got {:?}",
            row.case, created.status
        );
        assert_eq!(
            created.arn,
            format!(
                "{}/allocation/{}",
                expected_share_arn(catalog, &product),
                created.marketplace_revenue_share_allocation_id
            ),
            "case {:?}: expected the allocation ARN under its share, got {:?}",
            row.case,
            created.arn
        );
        // Each allocation write is a new revision of the parent share, so the
        // n-th allocation sits at revision n + 1.
        let expected_revision = (index + 2).to_string();
        assert_eq!(
            created.latest_marketplace_revenue_share_revision.as_deref(),
            Some(expected_revision.as_str()),
            "case {:?}: expected the parent share at revision {expected_revision}, got {:?}",
            row.case,
            created.latest_marketplace_revenue_share_revision
        );

        // Step 4: the allocation reads back field for field.
        let fetched: GetMarketplaceRevenueShareAllocationOutput = emulator
            .get_allocation(&GetMarketplaceRevenueShareAllocationInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                marketplace_revenue_share_allocation_id: created
                    .marketplace_revenue_share_allocation_id
                    .clone(),
                marketplace_revenue_share_revision: None,
            })
            .await
            .ok(row.case);
        assert_eq!(
            (
                fetched.effective_from.as_str(),
                fetched.effective_until.as_deref(),
                fetched.revenue_share_percent.as_str(),
                fetched.status.as_str(),
                fetched.arn.as_str(),
                fetched.created_date,
            ),
            (
                row.effective_from,
                row.effective_until,
                row.percent,
                "ACTIVE",
                created.arn.as_str(),
                created.created_date,
            ),
            "case {:?}: expected the stored allocation back, got {fetched:?}",
            row.case
        );
    }

    // Step 2 again: both counts are derived from the stored allocations.
    let share: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            revision: None,
        })
        .await
        .ok("the share after every allocation");
    let total = i32::try_from(rows.len()).expect("a small row count");
    assert_eq!(
        (
            share.revision,
            share.latest_revision,
            share.total_marketplace_revenue_share_allocation_count,
            share.total_active_marketplace_revenue_share_allocation_count,
        ),
        (Some(total + 1), Some(total + 1), Some(total), Some(total)),
        "expected {total} active allocations at revision {}, got {share:?}",
        total + 1
    );

    // A historical read reconstructs the share as it stood before the last
    // allocation was written.
    let earlier: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            revision: Some(2),
        })
        .await
        .ok("the share at revision 2");
    assert_eq!(
        (
            earlier.revision,
            earlier.latest_revision,
            earlier.total_marketplace_revenue_share_allocation_count,
        ),
        (Some(2), Some(total + 1), Some(1)),
        "expected exactly one allocation at revision 2, got {earlier:?}"
    );

    // A revision the share never reached names nothing.
    emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            revision: Some(total + 99),
        })
        .await
        .is_error(
            "a revision past the latest",
            404,
            "ResourceNotFoundException",
        );

    // An unknown allocation id under a known share is a 404, not an empty 200.
    emulator
        .get_allocation(&GetMarketplaceRevenueShareAllocationInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            marketplace_revenue_share_allocation_id: "mrsa-0000000000000".to_string(),
            marketplace_revenue_share_revision: None,
        })
        .await
        .is_error("an unknown allocation id", 404, "ResourceNotFoundException");
}

// ---------------------------------------------------------------------------
// The documented calendar and percentage rules.
// ---------------------------------------------------------------------------

/// One row of the refusal table: a create request, and why the service refuses
/// it.
struct RefusalRow {
    case: &'static str,
    effective_from: &'static str,
    effective_until: Option<&'static str>,
    percent: &'static str,
    /// The member the refusal must name.
    member: &'static str,
}

/// Every row breaks exactly one documented rule: `EffectiveFrom` "must be the
/// first day of a month", `EffectiveUntil` "must be the last day of a month",
/// and `RevenueSharePercent` is a share of revenue, so at most 100. Each value
/// satisfies the member's `@pattern`, so the refusal is the handler's.
fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "EffectiveFrom is mid-month",
            effective_from: "2024-01-15",
            effective_until: None,
            percent: "10",
            member: "EffectiveFrom",
        },
        RefusalRow {
            case: "EffectiveFrom is the last day of a month, not the first",
            effective_from: "2024-01-31",
            effective_until: None,
            percent: "10",
            member: "EffectiveFrom",
        },
        RefusalRow {
            case: "EffectiveFrom names a day February never has",
            effective_from: "2024-02-30",
            effective_until: None,
            percent: "10",
            member: "EffectiveFrom",
        },
        RefusalRow {
            case: "EffectiveFrom names month 13",
            effective_from: "2024-13-01",
            effective_until: None,
            percent: "10",
            member: "EffectiveFrom",
        },
        RefusalRow {
            case: "EffectiveUntil is mid-month",
            effective_from: "2024-01-01",
            effective_until: Some("2024-01-15"),
            percent: "10",
            member: "EffectiveUntil",
        },
        RefusalRow {
            case: "EffectiveUntil is the 29th of a February that has 28 days",
            effective_from: "2023-02-01",
            effective_until: Some("2023-02-29"),
            percent: "10",
            member: "EffectiveUntil",
        },
        RefusalRow {
            case: "EffectiveUntil ends before EffectiveFrom starts",
            effective_from: "2024-06-01",
            effective_until: Some("2024-01-31"),
            percent: "10",
            member: "EffectiveUntil",
        },
        RefusalRow {
            case: "RevenueSharePercent is one hundredth past 100",
            effective_from: "2024-01-01",
            effective_until: None,
            percent: "100.01",
            member: "RevenueSharePercent",
        },
        RefusalRow {
            case: "RevenueSharePercent is far past 100",
            effective_from: "2024-01-01",
            effective_until: None,
            percent: "999",
            member: "RevenueSharePercent",
        },
    ]
}

#[tokio::test]
async fn the_documented_calendar_and_percentage_rules_are_enforced() {
    let emulator = Emulator::new();
    let catalog = "AWS";
    let product = product_id("abc1234567890");
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&create_share_input(catalog, &product))
        .await
        .ok("the parent share");

    for row in refusal_rows() {
        let answer = emulator
            .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
                effective_until: row.effective_until.map(str::to_string),
                ..create_allocation_input(catalog, &product, row.effective_from, row.percent)
            })
            .await;
        answer.is_error(row.case, 400, "ValidationException");
        assert!(
            answer.mentions(row.member),
            "case {:?}: expected the refusal to name {:?}, got {}",
            row.case,
            row.member,
            answer.render()
        );
    }

    // The share stayed at revision 1: a refused create writes nothing.
    let share: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            revision: None,
        })
        .await
        .ok("the share after every refusal");
    assert_eq!(
        (
            share.revision,
            share.total_marketplace_revenue_share_allocation_count
        ),
        (Some(1), Some(0)),
        "expected a refused create to write nothing, got {share:?}"
    );

    // `Catalog` is an enum shape, so the handler owns its membership check.
    emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: "Production".to_string(),
            product_id: product.clone(),
            revision: None,
        })
        .await
        .is_error("a catalog the model does not declare", 400, "ValidationException");

    // An allocation under a product no share exists for is a 404.
    emulator
        .create_allocation(&create_allocation_input(
            catalog,
            &product_id("zzz0000000000"),
            "2024-01-01",
            "10",
        ))
        .await
        .is_error("an allocation under an unknown product", 404, "ResourceNotFoundException");
}

/// Product identifiers the model's `MarketplaceProductId` refuses: it is
/// `len 18..18` with `pattern ^prod-[a-z0-9]{13}$` (see the `ProductId` member
/// in every operation's contract header). One row per way a string can miss
/// that: absent, the wrong prefix, too short, too long, upper case, and a
/// non-alphanumeric suffix character.
fn invalid_product_ids() -> Vec<(&'static str, &'static str)> {
    vec![
        ("an absent product id", ""),
        ("no prod- prefix", "item-abc1234567890"),
        ("a suffix one character short", "prod-abc123456789"),
        ("a suffix one character long", "prod-abc12345678901"),
        ("an upper-case suffix", "prod-ABC1234567890"),
        ("a punctuation character in the suffix", "prod-abc123456789_"),
    ]
}

#[tokio::test]
async fn a_product_id_outside_the_modelled_pattern_is_refused_everywhere_it_appears() {
    let emulator = Emulator::new();
    let catalog = "AWS";

    for (case, product) in invalid_product_ids() {
        emulator
            .create_share(&create_share_input(catalog, product))
            .await
            .is_error(
                &format!("creating a share with {case}"),
                400,
                "ValidationException",
            );
        emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: catalog.to_string(),
                product_id: product.to_string(),
                revision: None,
            })
            .await
            .is_error(
                &format!("getting a share with {case}"),
                400,
                "ValidationException",
            );
        emulator
            .create_allocation(&create_allocation_input(
                catalog,
                product,
                "2099-01-01",
                "10",
            ))
            .await
            .is_error(
                &format!("creating an allocation under {case}"),
                400,
                "ValidationException",
            );
    }

    // And nothing was written: a store that refused every id above holds no
    // shares at all.
    let listed: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares(&ListMarketplaceRevenueSharesInput {
            catalog: catalog.to_string(),
            ..Default::default()
        })
        .await
        .ok("the store after every refused product id");
    assert!(
        listed.marketplace_revenue_share_summaries.is_empty(),
        "expected a refused product id to write nothing, got {:?}",
        listed.marketplace_revenue_share_summaries
    );
}

// ---------------------------------------------------------------------------
// Step 5 of the slice: filters, sorting and pagination.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_allocation_list_pages_filters_and_sorts_from_state() {
    let emulator = Emulator::new();
    let catalog = "AWS";
    let product = product_id("abc1234567890");
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&create_share_input(catalog, &product))
        .await
        .ok("the parent share");

    // One allocation per quarter of 2024, created in ascending date order.
    let months = ["2024-01-01", "2024-04-01", "2024-07-01", "2024-10-01"];
    let mut ids: Vec<String> = Vec::new();
    for (index, from) in months.iter().enumerate() {
        let created: CreateMarketplaceRevenueShareAllocationOutput = emulator
            .create_allocation(&create_allocation_input(
                catalog,
                &product,
                from,
                &format!("{}", index + 1),
            ))
            .await
            .ok(from);
        ids.push(created.marketplace_revenue_share_allocation_id);
    }

    // Every page size walks the whole collection exactly once, in the same
    // order, and only the last page omits `NextToken`.
    for page_size in [1_i32, 2, 3, 4, 7] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page: ListMarketplaceRevenueShareAllocationsOutput = emulator
                .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
                    catalog: catalog.to_string(),
                    product_id: product.clone(),
                    max_results: Some(page_size),
                    next_token: token.clone(),
                    sort_by: Some("EffectiveFrom".to_string()),
                    sort_order: Some("ASCENDING".to_string()),
                    ..Default::default()
                })
                .await
                .ok(&case);
            pages += 1;
            assert!(
                page.marketplace_revenue_share_allocation_summaries.len()
                    <= page_size as usize,
                "case {case:?}: expected at most {page_size} summaries, got {}",
                page.marketplace_revenue_share_allocation_summaries.len()
            );
            walked.extend(
                page.marketplace_revenue_share_allocation_summaries
                    .iter()
                    .map(|summary| summary.marketplace_revenue_share_allocation_id.clone()),
            );
            match page.next_token {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= months.len() + 2,
                "case {case:?}: the pagination token never ran out after {pages} pages"
            );
        }
        assert_eq!(
            walked, ids,
            "case {case:?}: expected every allocation once in EffectiveFrom order, got {walked:?}"
        );
    }

    // DESCENDING is the documented default for this list's `SortOrder`.
    let defaulted: ListMarketplaceRevenueShareAllocationsOutput = emulator
        .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            sort_by: Some("EffectiveFrom".to_string()),
            ..Default::default()
        })
        .await
        .ok("SortOrder absent");
    let descending: Vec<String> = defaulted
        .marketplace_revenue_share_allocation_summaries
        .iter()
        .map(|summary| summary.marketplace_revenue_share_allocation_id.clone())
        .collect();
    let mut expected = ids.clone();
    expected.reverse();
    assert_eq!(
        descending, expected,
        "expected the documented DESCENDING default, got {descending:?}"
    );

    // `AfterEffectiveFrom` is inclusive and `BeforeEffectiveFrom` exclusive: the
    // half-open range the model documents.
    struct RangeRow {
        case: &'static str,
        after: Option<&'static str>,
        before: Option<&'static str>,
        expected: &'static [usize],
    }
    let range_rows = [
        RangeRow {
            case: "no bound at all keeps everything",
            after: None,
            before: None,
            expected: &[0, 1, 2, 3],
        },
        RangeRow {
            case: "the inclusive lower bound keeps the allocation that equals it",
            after: Some("2024-04-01"),
            before: None,
            expected: &[1, 2, 3],
        },
        RangeRow {
            case: "the exclusive upper bound drops the allocation that equals it",
            after: None,
            before: Some("2024-07-01"),
            expected: &[0, 1],
        },
        RangeRow {
            case: "both bounds, one allocation between them",
            after: Some("2024-04-01"),
            before: Some("2024-07-01"),
            expected: &[1],
        },
        RangeRow {
            case: "an empty half-open range keeps nothing",
            after: Some("2024-04-01"),
            before: Some("2024-04-01"),
            expected: &[],
        },
    ];
    for row in range_rows {
        let page: ListMarketplaceRevenueShareAllocationsOutput = emulator
            .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                after_effective_from: row.after.map(str::to_string),
                before_effective_from: row.before.map(str::to_string),
                sort_by: Some("EffectiveFrom".to_string()),
                sort_order: Some("ASCENDING".to_string()),
                ..Default::default()
            })
            .await
            .ok(row.case);
        let got: Vec<String> = page
            .marketplace_revenue_share_allocation_summaries
            .iter()
            .map(|summary| summary.marketplace_revenue_share_allocation_id.clone())
            .collect();
        let want: Vec<String> = row.expected.iter().map(|index| ids[*index].clone()).collect();
        assert_eq!(
            got, want,
            "case {:?}: expected {want:?}, got {got:?}",
            row.case
        );
    }

    // A `MaxResults` that asks for no items at all is refused, not clamped.
    for requested in [0_i32, -1] {
        emulator
            .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                max_results: Some(requested),
                ..Default::default()
            })
            .await
            .is_error(
                &format!("MaxResults={requested}"),
                400,
                "ValidationException",
            );
    }

    // A token this query never issued is refused rather than silently restarted.
    emulator
        .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            next_token: Some("mrsa-9999999999999".to_string()),
            ..Default::default()
        })
        .await
        .is_error("a forged NextToken", 400, "ValidationException");

    // `ListMarketplaceRevenueShares` reads the same store: the share is there,
    // with the allocation counts the get reports.
    let shares: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares(&ListMarketplaceRevenueSharesInput {
            catalog: catalog.to_string(),
            ..Default::default()
        })
        .await
        .ok("the shares of the AWS catalog");
    assert_eq!(
        shares.marketplace_revenue_share_summaries.len(),
        1,
        "expected exactly the one created share, got {:?}",
        shares.marketplace_revenue_share_summaries
    );
    let summary = &shares.marketplace_revenue_share_summaries[0];
    assert_eq!(
        (
            summary.product_id.as_str(),
            summary.arn.as_str(),
            summary.total_marketplace_revenue_share_allocation_count,
            summary.total_active_marketplace_revenue_share_allocation_count,
        ),
        (
            product.as_str(),
            expected_share_arn(catalog, &product).as_str(),
            Some(4),
            Some(4),
        ),
        "expected the summary to report the stored share, got {summary:?}"
    );

    // The product-id filter keeps only what it names.
    let filtered: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares_raw(vec![
            ("Catalog", Cbor::text(catalog)),
            ("ProductIds", texts(&[&product_id("zzz0000000000")])),
        ])
        .await
        .ok("a product-id filter naming nothing stored");
    assert!(
        filtered.marketplace_revenue_share_summaries.is_empty(),
        "expected no summary, got {:?}",
        filtered.marketplace_revenue_share_summaries
    );
    let kept: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares_raw(vec![
            ("Catalog", Cbor::text(catalog)),
            (
                "ProductIds",
                texts(&[&product_id("zzz0000000000"), &product]),
            ),
        ])
        .await
        .ok("a product-id filter naming the stored share");
    assert_eq!(
        kept.marketplace_revenue_share_summaries.len(),
        1,
        "expected the named share, got {:?}",
        kept.marketplace_revenue_share_summaries
    );
    // The product-code filter reads the SAME derived code the summary reports.
    let by_code: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares_raw(vec![
            ("Catalog", Cbor::text(catalog)),
            (
                "ProductCodes",
                texts(&[summary.product_code.as_deref().unwrap_or_default()]),
            ),
        ])
        .await
        .ok("a product-code filter naming the stored share");
    assert_eq!(
        by_code.marketplace_revenue_share_summaries.len(),
        1,
        "expected the share with that product code, got {:?}",
        by_code.marketplace_revenue_share_summaries
    );
    let by_other_code: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares_raw(vec![
            ("Catalog", Cbor::text(catalog)),
            ("ProductCodes", texts(&["zzz0000000000"])),
        ])
        .await
        .ok("a product-code filter naming nothing stored");
    assert!(
        by_other_code
            .marketplace_revenue_share_summaries
            .is_empty(),
        "expected no summary, got {:?}",
        by_other_code.marketplace_revenue_share_summaries
    );

    // `CreatedAfter` and `CreatedBefore` are both STRICTLY exclusive and compare
    // at whole-second granularity — the model states neither bound's
    // inclusivity, and the handler says which convention it applies — so a share
    // created within the bound's own second is outside the window, and a second
    // either way puts it inside.
    let created = summary
        .created_date
        .expect("a stored share reports its CreatedDate");
    let created_second = created.secs();
    struct CreatedRow {
        case: &'static str,
        after: Option<i64>,
        before: Option<i64>,
        kept: bool,
    }
    for row in [
        CreatedRow {
            case: "CreatedAfter in the creation second is exclusive",
            after: Some(created_second),
            before: None,
            kept: false,
        },
        CreatedRow {
            case: "CreatedAfter one second earlier keeps the share",
            after: Some(created_second - 1),
            before: None,
            kept: true,
        },
        CreatedRow {
            case: "CreatedBefore in the creation second is exclusive",
            after: None,
            before: Some(created_second),
            kept: false,
        },
        CreatedRow {
            case: "CreatedBefore one second later keeps the share",
            after: None,
            before: Some(created_second + 1),
            kept: true,
        },
        CreatedRow {
            case: "both bounds straddling the creation second keep the share",
            after: Some(created_second - 1),
            before: Some(created_second + 1),
            kept: true,
        },
    ] {
        let page: ListMarketplaceRevenueSharesOutput = emulator
            .list_shares(&ListMarketplaceRevenueSharesInput {
                catalog: catalog.to_string(),
                created_after: row.after.map(aws_smithy_types::DateTime::from_secs),
                created_before: row.before.map(aws_smithy_types::DateTime::from_secs),
                ..Default::default()
            })
            .await
            .ok(row.case);
        assert_eq!(
            page.marketplace_revenue_share_summaries.len(),
            usize::from(row.kept),
            "case {:?}: expected {} summary, got {:?}",
            row.case,
            usize::from(row.kept),
            page.marketplace_revenue_share_summaries
        );
    }
}

// ---------------------------------------------------------------------------
// Step 6 of the slice: the update writes through, under the documented rules.
// ---------------------------------------------------------------------------

/// One row of the update table: which members the request changes.
struct UpdateRow {
    case: &'static str,
    effective_from: Option<&'static str>,
    effective_until: Option<&'static str>,
    percent: Option<&'static str>,
    status: Option<&'static str>,
}

fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "only the percentage",
            effective_from: None,
            effective_until: None,
            percent: Some("20"),
            status: None,
        },
        UpdateRow {
            case: "only the status, the model's soft delete",
            effective_from: None,
            effective_until: None,
            percent: None,
            status: Some("INACTIVE"),
        },
        UpdateRow {
            case: "the status back to ACTIVE",
            effective_from: None,
            effective_until: None,
            percent: None,
            status: Some("ACTIVE"),
        },
        UpdateRow {
            case: "only the end date",
            effective_from: None,
            effective_until: Some("2099-06-30"),
            percent: None,
            status: None,
        },
        UpdateRow {
            case: "the start date, which must stay a first-of-month",
            effective_from: Some("2099-02-01"),
            effective_until: None,
            percent: None,
            status: None,
        },
        UpdateRow {
            case: "every mutable member at once",
            effective_from: Some("2099-03-01"),
            effective_until: Some("2099-09-30"),
            percent: Some("5.75"),
            status: Some("INACTIVE"),
        },
    ]
}

#[tokio::test]
async fn an_update_writes_through_and_honours_the_revision_check() {
    let emulator = Emulator::new();
    let catalog = "AWS";
    let product = product_id("abc1234567890");
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&create_share_input(catalog, &product))
        .await
        .ok("the parent share");
    // Future-dated, because the model says `EffectiveFrom`,
    // `RevenueSharePercent` and `Status` are "Only modifiable on future-dated
    // allocations".
    let created: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
            effective_until: Some(FUTURE_UNTIL.to_string()),
            ..create_allocation_input(catalog, &product, FUTURE_FROM, "12.5")
        })
        .await
        .ok("a future-dated allocation");
    let allocation = created.marketplace_revenue_share_allocation_id.clone();

    let mut expected_from = FUTURE_FROM.to_string();
    let mut expected_until = Some(FUTURE_UNTIL.to_string());
    let mut expected_percent = "12.5".to_string();
    let mut expected_status = "ACTIVE".to_string();
    // The share is at revision 2: 1 for the create, 2 for the allocation.
    let mut revision = 2_i32;

    for row in update_rows() {
        let updated: UpdateMarketplaceRevenueShareAllocationOutput = emulator
            .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                marketplace_revenue_share_allocation_id: allocation.clone(),
                marketplace_revenue_share_revision: revision.to_string(),
                effective_from: row.effective_from.map(str::to_string),
                effective_until: row.effective_until.map(str::to_string),
                revenue_share_percent: row.percent.map(str::to_string),
                status: row.status.map(str::to_string),
                client_token: None,
            })
            .await
            .ok(row.case);

        if let Some(from) = row.effective_from {
            expected_from = from.to_string();
        }
        if let Some(until) = row.effective_until {
            expected_until = Some(until.to_string());
        }
        if let Some(percent) = row.percent {
            expected_percent = percent.to_string();
        }
        if let Some(status) = row.status {
            expected_status = status.to_string();
        }
        revision += 1;

        assert_eq!(
            (
                updated.effective_from.as_str(),
                updated.effective_until.as_deref(),
                updated.revenue_share_percent.as_str(),
                updated.status.as_str(),
                updated
                    .latest_marketplace_revenue_share_revision
                    .as_deref(),
            ),
            (
                expected_from.as_str(),
                expected_until.as_deref(),
                expected_percent.as_str(),
                expected_status.as_str(),
                Some(revision.to_string().as_str()),
            ),
            "case {:?}: expected the update to answer the new values, got {updated:?}",
            row.case
        );

        // THIS output's `Arn` is documented as the PARENT share's ARN, where the
        // create and get outputs document theirs as the allocation's own. Every
        // row asserts it, so no update answers the allocation ARN here.
        assert_eq!(
            updated.arn,
            expected_share_arn(catalog, &product),
            "case {:?}: expected the parent share ARN on the update output, got {:?}",
            row.case,
            updated.arn
        );

        // The following read observes exactly the change.
        let fetched: GetMarketplaceRevenueShareAllocationOutput = emulator
            .get_allocation(&GetMarketplaceRevenueShareAllocationInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                marketplace_revenue_share_allocation_id: allocation.clone(),
                marketplace_revenue_share_revision: None,
            })
            .await
            .ok(row.case);
        assert_eq!(
            (
                fetched.effective_from.as_str(),
                fetched.effective_until.as_deref(),
                fetched.revenue_share_percent.as_str(),
                fetched.status.as_str(),
            ),
            (
                expected_from.as_str(),
                expected_until.as_deref(),
                expected_percent.as_str(),
                expected_status.as_str(),
            ),
            "case {:?}: expected the get to observe the update, got {fetched:?}",
            row.case
        );

        // And so does the list.
        let listed: ListMarketplaceRevenueShareAllocationsOutput = emulator
            .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                ..Default::default()
            })
            .await
            .ok(row.case);
        let summary = listed
            .marketplace_revenue_share_allocation_summaries
            .first()
            .unwrap_or_else(|| panic!("case {:?}: expected one summary", row.case));
        assert_eq!(
            (
                summary.revenue_share_percent.as_str(),
                summary.status.as_str()
            ),
            (expected_percent.as_str(), expected_status.as_str()),
            "case {:?}: expected the list to observe the update, got {summary:?}",
            row.case
        );

        // The `Status` filter reads the stored status: the allocation appears
        // under its own status and nowhere else.
        for filter in ["ACTIVE", "INACTIVE"] {
            let filtered: ListMarketplaceRevenueShareAllocationsOutput = emulator
                .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
                    catalog: catalog.to_string(),
                    product_id: product.clone(),
                    status: Some(filter.to_string()),
                    ..Default::default()
                })
                .await
                .ok(row.case);
            let want = usize::from(filter == expected_status);
            assert_eq!(
                filtered
                    .marketplace_revenue_share_allocation_summaries
                    .len(),
                want,
                "case {:?}: expected {want} summary with Status={filter} while the stored \
                 status is {expected_status}, got {:?}",
                row.case,
                filtered.marketplace_revenue_share_allocation_summaries
            );
        }

        // The active count follows the status, because it is counted and not
        // stored.
        let share: GetMarketplaceRevenueShareOutput = emulator
            .get_share(&GetMarketplaceRevenueShareInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                revision: None,
            })
            .await
            .ok(row.case);
        let expected_active = i32::from(expected_status == "ACTIVE");
        assert_eq!(
            (
                share.revision,
                share.total_marketplace_revenue_share_allocation_count,
                share.total_active_marketplace_revenue_share_allocation_count,
            ),
            (Some(revision), Some(1), Some(expected_active)),
            "case {:?}: expected the counts to follow the stored status, got {share:?}",
            row.case
        );
    }

    // A status the model does not declare is refused rather than ignored.
    emulator
        .list_allocations(&ListMarketplaceRevenueShareAllocationsInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            status: Some("ARCHIVED".to_string()),
            ..Default::default()
        })
        .await
        .is_error(
            "a Status the model does not declare",
            400,
            "ValidationException",
        );

    // A historical read at the revision the allocation was created at answers
    // the values the create wrote, not the ones the updates left.
    let original: GetMarketplaceRevenueShareAllocationOutput = emulator
        .get_allocation(&GetMarketplaceRevenueShareAllocationInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            marketplace_revenue_share_allocation_id: allocation.clone(),
            marketplace_revenue_share_revision: Some("2".to_string()),
        })
        .await
        .ok("the allocation at revision 2");
    assert_eq!(
        (
            original.effective_from.as_str(),
            original.effective_until.as_deref(),
            original.revenue_share_percent.as_str(),
            original.status.as_str(),
        ),
        (FUTURE_FROM, Some(FUTURE_UNTIL), "12.5", "ACTIVE"),
        "expected the created version at revision 2, got {original:?}"
    );

    // The optimistic concurrency check: a revision that is no longer current.
    for stale in [1_i32, 2, revision - 1, revision + 1] {
        let answer = emulator
            .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                marketplace_revenue_share_allocation_id: allocation.clone(),
                marketplace_revenue_share_revision: stale.to_string(),
                revenue_share_percent: Some("1".to_string()),
                ..Default::default()
            })
            .await;
        answer.is_error(
            &format!("MarketplaceRevenueShareRevision={stale}"),
            409,
            "ConflictException",
        );
    }

    // An id naming no allocation is a 404 whatever the revision says: the
    // resource is resolved BEFORE the concurrency check, so a caller is told
    // what is actually wrong.
    for (case, supplied) in [
        ("an unknown allocation id at the current revision", revision),
        ("an unknown allocation id at a stale revision", 1),
    ] {
        emulator
            .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                marketplace_revenue_share_allocation_id: "mrsa-0000000000000".to_string(),
                marketplace_revenue_share_revision: supplied.to_string(),
                revenue_share_percent: Some("1".to_string()),
                ..Default::default()
            })
            .await
            .is_error(case, 404, "ResourceNotFoundException");
    }

    // An update that changes nothing is refused rather than minting a revision
    // for nothing.
    emulator
        .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            marketplace_revenue_share_allocation_id: allocation.clone(),
            marketplace_revenue_share_revision: revision.to_string(),
            ..Default::default()
        })
        .await
        .is_error("an update with no mutable member", 400, "ValidationException");

    // And the share did not move under any of those refusals.
    let share: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            revision: None,
        })
        .await
        .ok("the share after every refused update");
    assert_eq!(
        share.revision,
        Some(revision),
        "expected a refused update to write nothing, got {share:?}"
    );
}

#[tokio::test]
async fn an_in_force_allocation_keeps_the_members_the_model_protects() {
    let emulator = Emulator::new();
    let catalog = "Sandbox";
    let product = product_id("abc1234567890");
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&create_share_input(catalog, &product))
        .await
        .ok("the parent share");
    let created: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(catalog, &product, PAST_FROM, "10"))
        .await
        .ok("an allocation already in force");
    let allocation = created.marketplace_revenue_share_allocation_id.clone();

    // "Only modifiable on future-dated allocations" — one row per protected
    // member.
    let protected: [(&str, UpdateMarketplaceRevenueShareAllocationInput); 3] = [
        (
            "EffectiveFrom",
            UpdateMarketplaceRevenueShareAllocationInput {
                effective_from: Some("2098-01-01".to_string()),
                ..Default::default()
            },
        ),
        (
            "RevenueSharePercent",
            UpdateMarketplaceRevenueShareAllocationInput {
                revenue_share_percent: Some("20".to_string()),
                ..Default::default()
            },
        ),
        (
            "Status",
            UpdateMarketplaceRevenueShareAllocationInput {
                status: Some("INACTIVE".to_string()),
                ..Default::default()
            },
        ),
    ];
    for (member, change) in protected {
        let answer = emulator
            .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
                catalog: catalog.to_string(),
                product_id: product.clone(),
                marketplace_revenue_share_allocation_id: allocation.clone(),
                marketplace_revenue_share_revision: "2".to_string(),
                ..change
            })
            .await;
        answer.is_error(member, 400, "ValidationException");
        assert!(
            answer.mentions(member),
            "case {member:?}: expected the refusal to name the protected member, got {}",
            answer.render()
        );
    }

    // `EffectiveUntil` stays modifiable, which is how an in-force allocation is
    // ended — as long as the new end is a month end on or after today.
    let ended: UpdateMarketplaceRevenueShareAllocationOutput = emulator
        .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            marketplace_revenue_share_allocation_id: allocation.clone(),
            marketplace_revenue_share_revision: "2".to_string(),
            effective_until: Some(FUTURE_UNTIL.to_string()),
            ..Default::default()
        })
        .await
        .ok("ending an in-force allocation");
    assert_eq!(
        (
            ended.effective_until.as_deref(),
            ended.revenue_share_percent.as_str(),
            ended.status.as_str(),
        ),
        (Some(FUTURE_UNTIL), "10", "ACTIVE"),
        "expected only the end date to change, got {ended:?}"
    );

    // An end date before today is refused: the model says "on or after today".
    emulator
        .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            marketplace_revenue_share_allocation_id: allocation.clone(),
            marketplace_revenue_share_revision: "3".to_string(),
            effective_until: Some("2020-01-31".to_string()),
            ..Default::default()
        })
        .await
        .is_error("an end date in the past", 400, "ValidationException");
}

// ---------------------------------------------------------------------------
// Idempotency and conflicts.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_idempotency_token_replays_instead_of_creating_a_duplicate() {
    let emulator = Emulator::new();
    let catalog = "AWS";
    let product = product_id("abc1234567890");

    let first: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            client_token: Some("tok-1".to_string()),
            ..create_share_input(catalog, &product)
        })
        .await
        .ok("the first create");
    let replay: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            client_token: Some("tok-1".to_string()),
            ..create_share_input(catalog, &product)
        })
        .await
        .ok("the replayed create");
    assert_eq!(
        (replay.arn.as_str(), replay.revision),
        (first.arn.as_str(), first.revision),
        "expected the replay to answer the first share, got {replay:?}"
    );

    // Every `ClientToken` the model admits replays the same way; the recorded
    // `tok-1` is one row, and these span the pattern's length bounds and its
    // punctuation range. Each row carries its own product, because one token per
    // product is what a client sends.
    for (index, token) in ["!", "~", &"t".repeat(64), "a-b_c.d/e:f"]
        .into_iter()
        .enumerate()
    {
        let own_product = product_id(&format!("{:013}", index + 10));
        let created: CreateMarketplaceRevenueShareOutput = emulator
            .create_share(&CreateMarketplaceRevenueShareInput {
                client_token: Some(token.to_string()),
                ..create_share_input(catalog, &own_product)
            })
            .await
            .ok(token);
        let replayed: CreateMarketplaceRevenueShareOutput = emulator
            .create_share(&CreateMarketplaceRevenueShareInput {
                client_token: Some(token.to_string()),
                ..create_share_input(catalog, &own_product)
            })
            .await
            .ok(token);
        assert_eq!(
            (replayed.arn.as_str(), replayed.revision),
            (created.arn.as_str(), created.revision),
            "case {token:?}: expected the replay to answer the first share, got {replayed:?}"
        );
    }

    // A second create of the same ProductId under a DIFFERENT token conflicts
    // rather than overwriting.
    emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            client_token: Some("tok-other".to_string()),
            ..create_share_input(catalog, &product)
        })
        .await
        .is_error("a different token for the same product", 409, "ConflictException");

    // The same token asking for a DIFFERENT share conflicts too.
    emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            client_token: Some("tok-1".to_string()),
            ..create_share_input(catalog, &product_id("0000000000002"))
        })
        .await
        .is_error("one token, two products", 409, "ConflictException");

    // One share per product, and not one more: a replay creates nothing.
    let shares: ListMarketplaceRevenueSharesOutput = emulator
        .list_shares(&ListMarketplaceRevenueSharesInput {
            catalog: catalog.to_string(),
            ..Default::default()
        })
        .await
        .ok("the shares after the idempotent replays");
    assert_eq!(
        shares.marketplace_revenue_share_summaries.len(),
        5,
        "expected one share per product and no duplicate, got {:?}",
        shares.marketplace_revenue_share_summaries
    );

    // The allocation create replays the same way.
    let allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
            client_token: Some("alloc-1".to_string()),
            ..create_allocation_input(catalog, &product, "2024-01-01", "12.5")
        })
        .await
        .ok("the first allocation create");
    let replay: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
            client_token: Some("alloc-1".to_string()),
            ..create_allocation_input(catalog, &product, "2024-01-01", "12.5")
        })
        .await
        .ok("the replayed allocation create");
    assert_eq!(
        replay.marketplace_revenue_share_allocation_id,
        allocation.marketplace_revenue_share_allocation_id,
        "expected the replay to answer the first allocation, got {replay:?}"
    );
    emulator
        .create_allocation(&CreateMarketplaceRevenueShareAllocationInput {
            client_token: Some("alloc-1".to_string()),
            ..create_allocation_input(catalog, &product, "2024-02-01", "12.5")
        })
        .await
        .is_error(
            "one allocation token, two effective dates",
            409,
            "ConflictException",
        );

    // The share moved exactly once for the one allocation that was written.
    let share: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            revision: None,
        })
        .await
        .ok("the share after the allocation replays");
    assert_eq!(
        (
            share.revision,
            share.total_marketplace_revenue_share_allocation_count
        ),
        (Some(2), Some(1)),
        "expected one allocation and one revision bump, got {share:?}"
    );
}

#[tokio::test]
async fn a_replayed_update_answers_the_version_it_already_wrote() {
    let emulator = Emulator::new();
    let catalog = "AWS";
    let product = product_id("abc1234567890");
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&create_share_input(catalog, &product))
        .await
        .ok("the parent share");
    let created: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(
            catalog, &product, FUTURE_FROM, "12.5",
        ))
        .await
        .ok("a future-dated allocation");
    let allocation = created.marketplace_revenue_share_allocation_id.clone();

    let request = UpdateMarketplaceRevenueShareAllocationInput {
        catalog: catalog.to_string(),
        product_id: product.clone(),
        marketplace_revenue_share_allocation_id: allocation.clone(),
        marketplace_revenue_share_revision: "2".to_string(),
        client_token: Some("upd-1".to_string()),
        revenue_share_percent: Some("20".to_string()),
        ..Default::default()
    };
    let first: UpdateMarketplaceRevenueShareAllocationOutput =
        emulator.update_allocation(&request).await.ok("the update");
    // The identical retry must not fail the optimistic concurrency check that
    // the first attempt itself moved past, and must not mint a second revision.
    let replay: UpdateMarketplaceRevenueShareAllocationOutput = emulator
        .update_allocation(&request)
        .await
        .ok("the retried update");
    assert_eq!(
        (
            replay.revenue_share_percent.as_str(),
            replay
                .latest_marketplace_revenue_share_revision
                .as_deref(),
        ),
        (
            first.revenue_share_percent.as_str(),
            first.latest_marketplace_revenue_share_revision.as_deref(),
        ),
        "expected the retry to answer the version the first attempt wrote, got {replay:?}"
    );

    let share: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            revision: None,
        })
        .await
        .ok("the share after the retried update");
    assert_eq!(
        share.revision,
        Some(3),
        "expected one revision for the create, one for the allocation and one for \
         the single update, got {share:?}"
    );
}

// ---------------------------------------------------------------------------
// The audit record every state-changing operation owes.
// ---------------------------------------------------------------------------

/// One captured `tracing` event: the target it was emitted on and its fields.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: std::collections::BTreeMap<String, String>,
}

/// Collects an event's fields as strings, whatever type each was recorded as.
#[derive(Default)]
struct FieldCollector(std::collections::BTreeMap<String, String>);

impl tracing::field::Visit for FieldCollector {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A subscriber that keeps every event it is handed, so this suite can assert on
/// the audit records the service emits. Written over `tracing` itself, which this
/// crate already depends on.
struct CapturingSubscriber {
    events: Arc<std::sync::Mutex<Vec<CapturedEvent>>>,
}

impl tracing::Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = FieldCollector::default();
        event.record(&mut fields);
        self.events
            .lock()
            .expect("the captured events")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields: fields.0,
            });
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The sink the capturing subscriber writes into: every record this test binary
/// emits, from any suite running in parallel, which is why the assertions match
/// each record by the resource it names.
static CAPTURED_EVENTS: std::sync::OnceLock<Arc<std::sync::Mutex<Vec<CapturedEvent>>>> =
    std::sync::OnceLock::new();

/// Install the capturing subscriber as this process's default, once.
fn install_the_audit_capture() -> Arc<std::sync::Mutex<Vec<CapturedEvent>>> {
    let events = Arc::clone(
        CAPTURED_EVENTS.get_or_init(|| Arc::new(std::sync::Mutex::new(Vec::new()))),
    );
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let subscriber = CapturingSubscriber {
            events: Arc::clone(&events),
        };
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
    events
}

/// The audit records captured so far.
fn captured_audit_records() -> Vec<CapturedEvent> {
    let events = install_the_audit_capture();
    let events = events.lock().expect("the captured events");
    events
        .iter()
        .filter(|event| event.target == prm::state::AUDIT_TARGET)
        .cloned()
        .collect()
}

/// Every operation that CHANGES stored state emits the structured audit record
/// it owes — the operation, the resource it changed, the outcome, the Region and
/// the access key id the caller presented — and no secret alongside it. A read
/// changes nothing, so it owes no record.
#[tokio::test]
async fn state_changing_operations_emit_an_audit_record() {
    install_the_audit_capture();
    let emulator = Emulator::new();
    let catalog = "AWS";
    // A product id no other suite uses, so each record below is this test's.
    let product = product_id("aud1234567890");
    let share: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            client_token: Some("audit-share".to_string()),
            ..create_share_input(catalog, &product)
        })
        .await
        .ok("the share create");
    // The idempotent re-send writes nothing, and says so.
    let _: CreateMarketplaceRevenueShareOutput = emulator
        .create_share(&CreateMarketplaceRevenueShareInput {
            client_token: Some("audit-share".to_string()),
            ..create_share_input(catalog, &product)
        })
        .await
        .ok("the replayed share create");
    let allocation: CreateMarketplaceRevenueShareAllocationOutput = emulator
        .create_allocation(&create_allocation_input(
            catalog, &product, FUTURE_FROM, "12.5",
        ))
        .await
        .ok("the allocation create");
    let _: UpdateMarketplaceRevenueShareAllocationOutput = emulator
        .update_allocation(&UpdateMarketplaceRevenueShareAllocationInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            marketplace_revenue_share_allocation_id: allocation
                .marketplace_revenue_share_allocation_id
                .clone(),
            marketplace_revenue_share_revision: "2".to_string(),
            revenue_share_percent: Some("20".to_string()),
            ..Default::default()
        })
        .await
        .ok("the allocation update");
    // A read: it owes no record, which the count below pins.
    let _: GetMarketplaceRevenueShareOutput = emulator
        .get_share(&GetMarketplaceRevenueShareInput {
            catalog: catalog.to_string(),
            product_id: product.clone(),
            revision: None,
        })
        .await
        .ok("the share read");

    let records = captured_audit_records();
    let mine: Vec<&CapturedEvent> = records
        .iter()
        .filter(|record| {
            record
                .fields
                .get("resource")
                .is_some_and(|resource| resource.contains(&product))
        })
        .collect();

    // (case, the operation, the resource it names, the outcome)
    for (case, operation, resource, outcome) in [
        (
            "the share create",
            "CreateMarketplaceRevenueShare",
            share.arn.clone(),
            "created",
        ),
        (
            "the replayed share create",
            "CreateMarketplaceRevenueShare",
            share.arn.clone(),
            "replayed: no change",
        ),
        (
            "the allocation create",
            "CreateMarketplaceRevenueShareAllocation",
            allocation.arn.clone(),
            "created",
        ),
        (
            "the allocation update",
            "UpdateMarketplaceRevenueShareAllocation",
            allocation.arn.clone(),
            "updated",
        ),
    ] {
        let matching: Vec<&&CapturedEvent> = mine
            .iter()
            .filter(|record| {
                record.fields.get("operation").map(String::as_str) == Some(operation)
                    && record.fields.get("resource") == Some(&resource)
                    && record.fields.get("outcome").map(String::as_str) == Some(outcome)
            })
            .collect();
        assert!(
            !matching.is_empty(),
            "case {case:?}: no audit record for {operation} on {resource} with outcome \
             {outcome:?}, got {mine:?}"
        );
        for record in matching {
            assert_eq!(
                record.fields.get("access_key_id").map(String::as_str),
                Some("AKIDEMULATOR"),
                "case {case:?}: the record must name the presented credential, got {record:?}"
            );
            assert_eq!(
                record.fields.get("region").map(String::as_str),
                Some(REGION),
                "case {case:?}: the record must name the Region, got {record:?}"
            );
        }
    }
    assert_eq!(
        mine.len(),
        4,
        "expected one record per state-changing call and none for the read, got {mine:?}"
    );
    assert!(
        !mine.iter().any(|record| {
            record
                .fields
                .values()
                .any(|value| value.contains("Signature") || value.contains("AWS4-HMAC"))
        }),
        "an audit record must carry no signature, got {mine:?}"
    );
}
