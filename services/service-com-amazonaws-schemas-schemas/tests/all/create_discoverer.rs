// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `CreateDiscoverer`.
//!
//! RULE: `CreateDiscoverer` MINTS a discoverer in this service's store and answers from
//! the resource it stored — the id and the ARN are the service's own (the ARN naming this
//! service, the region the request was signed for and the minted id), every other member
//! is what the request asked for or the member's modelled default, and a request outside
//! the modelled bounds of its input is refused with HTTP 400 and stores nothing at all,
//! so it leaves the store exactly as it found it and consumes no id.
//!
//! The rule is asserted over TABLES whose rows differ along what it generalises over: the
//! length of `SourceArn` at and past each published bound of `__stringMin20Max1600`, the
//! length of `Description` at and past each published bound of `__stringMin0Max256`
//! (source for both: `smithy_query(query="operation", operation="CreateDiscoverer")`), and
//! the region the create is signed for against the partition, region and account of the
//! source being discovered.
//!
//! The round-trip of a stored discoverer's members and the refusals on the FORM of
//! `SourceArn` are pinned in `discoverer_state`; what this suite adds is the boundary of
//! the modelled constraints, the proof that a refused create writes nothing, and the
//! independence of the minted ARN from the source it discovers.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`; the
//! discoverer routes and the create helper come from `discoverer_state`.

use serde_json::{json, Value};

use crate::discoverer_state::{
    create_discoverer, discoverer_uri, source_arn, state_of, DISCOVERERS_URI,
};
use crate::registry_schema_lifecycle::{Emulator, ACCOUNT, REGION};

/// The fixed part of the source ARNs this suite builds by length: the `arn` scheme and
/// the five colons every ARN has, with the partition, region and account fields left
/// blank (which an ARN of a service-wide resource does).
const SOURCE_ARN_PREFIX: &str = "arn:aws:events:::";

/// A well-formed source ARN of exactly `length` characters.
///
/// The resource field is padded to reach the length the row asks for, so the ONLY thing
/// a row varies is the length the model bounds: every value this returns is an ARN, and
/// a refusal of one can therefore only be about its length.
fn source_arn_of_length(length: usize) -> String {
    assert!(
        length > SOURCE_ARN_PREFIX.len(),
        "a source ARN of {length} characters leaves no room for a resource field"
    );
    format!(
        "{SOURCE_ARN_PREFIX}{}",
        "b".repeat(length - SOURCE_ARN_PREFIX.len())
    )
}

/// One `CreateDiscoverer` request whose input sits at, or past, a modelled bound.
#[derive(Debug)]
struct BoundCase {
    case: &'static str,
    source_arn: String,
    /// The `Description` the request sends, or `None` to omit the member.
    description: Option<String>,
    /// The status this request must be answered with: 201 for a value the model admits,
    /// 400 for one it does not.
    expected_status: u16,
}

impl BoundCase {
    /// The request body this row sends.
    fn body(&self) -> Value {
        let mut body = json!({"SourceArn": self.source_arn});
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        body
    }
}

/// The table. Rows differ along the length of each bounded input member — at each
/// published bound, one character past it, and far past it. The bounds are the model's:
/// `SourceArn` targets `__stringMin20Max1600` (len 20..1600) and `Description` targets
/// `__stringMin0Max256` (len 0..256), per
/// `smithy_query(query="operation", operation="CreateDiscoverer")`.
fn bound_cases() -> Vec<BoundCase> {
    vec![
        BoundCase {
            case: "a source ARN at the modelled lower bound of 20",
            source_arn: source_arn_of_length(20),
            description: None,
            expected_status: 201,
        },
        BoundCase {
            case: "a source ARN one character below the lower bound",
            source_arn: source_arn_of_length(19),
            description: None,
            expected_status: 400,
        },
        BoundCase {
            case: "a source ARN far below the lower bound",
            source_arn: source_arn_of_length(18),
            description: None,
            expected_status: 400,
        },
        BoundCase {
            case: "a source ARN at the modelled upper bound of 1600",
            source_arn: source_arn_of_length(1600),
            description: None,
            expected_status: 201,
        },
        BoundCase {
            case: "a source ARN one character past the upper bound",
            source_arn: source_arn_of_length(1601),
            description: None,
            expected_status: 400,
        },
        BoundCase {
            case: "a source ARN far past the upper bound",
            source_arn: source_arn_of_length(4_096),
            description: None,
            expected_status: 400,
        },
        BoundCase {
            case: "a description at the modelled lower bound of 0",
            source_arn: source_arn("empty-description"),
            description: Some(String::new()),
            expected_status: 201,
        },
        BoundCase {
            case: "a description at the modelled upper bound of 256",
            source_arn: source_arn("full-description"),
            description: Some("d".repeat(256)),
            expected_status: 201,
        },
        BoundCase {
            case: "a description one character past the upper bound",
            source_arn: source_arn("over-description"),
            description: Some("d".repeat(257)),
            expected_status: 400,
        },
        BoundCase {
            case: "a description far past the upper bound",
            source_arn: source_arn("huge-description"),
            description: Some("d".repeat(4_096)),
            expected_status: 400,
        },
    ]
}

/// RULE: a create whose input the model admits mints a discoverer that reads back with
/// exactly the members it was given, and one whose input is outside a modelled bound is
/// refused with HTTP 400 and stores nothing — so the store is left able to mint the very
/// id a store that had never been asked would mint.
#[tokio::test]
async fn input_at_the_modelled_bounds_is_stored_and_input_past_them_stores_nothing() {
    // The id the FIRST discoverer of an untouched store is minted with. A refused create
    // must not consume it: that is what "nothing was stored" means for an operation whose
    // whole effect is the resource, and the minted id, it adds.
    let untouched_store_mints = {
        let emulator = Emulator::new();
        create_discoverer(&emulator, "an untouched store", "first").await
    };

    for case in bound_cases() {
        // One store per row, so what a row observes is the effect of its own request.
        let emulator = Emulator::new();
        let created = emulator
            .call("POST", DISCOVERERS_URI, Some(case.body()))
            .await;
        assert_eq!(
            created.status, case.expected_status,
            "case {:?}: expected {}, got {} {:?}",
            case.case, case.expected_status, created.status, created.body
        );

        if case.expected_status == 201 {
            let discoverer_id = created.str_at("DiscovererId");
            let described = emulator
                .call("GET", &discoverer_uri(&discoverer_id), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {:?}: expected 200 from DescribeDiscoverer, got {} {:?}",
                case.case, described.status, described.body
            );
            assert_eq!(
                described.str_at("SourceArn"),
                case.source_arn,
                "case {:?}: the stored SourceArn must be the source the request named",
                case.case
            );
            assert_eq!(
                described.body.get("Description").and_then(Value::as_str),
                case.description.as_deref(),
                "case {:?}: expected the stored Description {:?}, got {:?}",
                case.case,
                case.description,
                described.body.get("Description")
            );
            assert_eq!(
                state_of(&described, case.case),
                "STARTED",
                "case {:?}: a created discoverer is discovering, got {:?}",
                case.case,
                described.body
            );
            continue;
        }

        assert_eq!(
            created.error_type(),
            "ValidationException",
            "case {:?}: a value outside a modelled bound is refused by the boundary \
             validator, got {:?}",
            case.case,
            created.header("x-amzn-errortype")
        );
        let minted_after = create_discoverer(&emulator, case.case, "first").await;
        assert_eq!(
            minted_after, untouched_store_mints,
            "case {:?}: a refused create must store nothing and consume no id, so the \
             next create mints {untouched_store_mints:?}, got {minted_after:?}",
            case.case
        );
    }
}

/// One create, signed for one region, of a source that lives somewhere else entirely.
#[derive(Debug)]
struct RegionCase {
    case: &'static str,
    /// The region the create is signed for — the region the minted ARN must carry.
    signed_for: &'static str,
    /// The event source being discovered, whose own partition, region and account have
    /// nothing to do with the ARN this service mints.
    source_arn: &'static str,
}

/// The table. Rows differ along the region the request is signed for and along the
/// partition, region and account fields of the source ARN — including rows where the two
/// regions disagree and where the source is in another partition and another account.
fn region_cases() -> Vec<RegionCase> {
    vec![
        RegionCase {
            case: "a source in the region the request is signed for",
            signed_for: REGION,
            source_arn: "arn:aws:events:us-west-2:123456789012:event-bus/same-region",
        },
        RegionCase {
            case: "a source in another region",
            signed_for: "eu-central-1",
            source_arn: "arn:aws:events:us-east-1:210987654321:event-bus/other-region",
        },
        RegionCase {
            case: "a source in another partition and account",
            signed_for: "ap-southeast-1",
            source_arn: "arn:aws-cn:events:cn-north-1:210987654321:event-bus/other-partition",
        },
        RegionCase {
            case: "a source whose region and account fields are blank",
            signed_for: "us-east-2",
            source_arn: "arn:aws:events:::event-bus/global",
        },
    ]
}

/// RULE: the ARN a create mints is THIS service's ARN for the discoverer it stored — it
/// names this service, the region the request was signed for and the minted id, never any
/// field of the source being discovered — and it is a stored property of the resource, so
/// every later read reports the ARN the create minted whatever region the read is signed
/// for.
#[tokio::test]
async fn the_minted_arn_names_this_service_the_signed_region_and_the_minted_id() {
    let emulator = Emulator::new();
    let mut minted = Vec::new();

    for case in region_cases() {
        let created = emulator
            .call_signed_for(
                case.signed_for,
                "POST",
                DISCOVERERS_URI,
                Some(json!({"SourceArn": case.source_arn})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.case, created.status, created.body
        );

        let discoverer_id = created.str_at("DiscovererId");
        let expected_arn = format!(
            "arn:aws:schemas:{}:{ACCOUNT}:discoverer/{discoverer_id}",
            case.signed_for
        );
        assert_eq!(
            created.str_at("DiscovererArn"),
            expected_arn,
            "case {:?}: expected the ARN {expected_arn}, got {:?}",
            case.case,
            created.body.get("DiscovererArn")
        );
        assert_eq!(
            created.str_at("SourceArn"),
            case.source_arn,
            "case {:?}: the source is reported as the request named it",
            case.case
        );
        minted.push((case, discoverer_id, expected_arn));
    }

    // Every discoverer still reports the ARN its own create minted, read from a region
    // that is not the one it was created in: the ARN is stored, not re-derived per read.
    for (case, discoverer_id, expected_arn) in &minted {
        for reading_region in ["us-west-1", "eu-west-3"] {
            let described = emulator
                .call_signed_for(
                    reading_region,
                    "GET",
                    &discoverer_uri(discoverer_id),
                    None,
                )
                .await;
            assert_eq!(
                described.status, 200,
                "case {:?}: expected 200 reading from {reading_region}, got {} {:?}",
                case.case, described.status, described.body
            );
            assert_eq!(
                &described.str_at("DiscovererArn"),
                expected_arn,
                "case {:?}: a read from {reading_region} must report the stored ARN \
                 {expected_arn}, got {:?}",
                case.case,
                described.body.get("DiscovererArn")
            );
        }
    }
}
