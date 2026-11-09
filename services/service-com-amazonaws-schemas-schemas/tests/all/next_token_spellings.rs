// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for which SPELLINGS of a `NextToken` resume a listing of this
//! service.
//!
//! RULE under test: a `NextToken` is an opaque label the SERVICE renders, so a value
//! names the position it encodes only when it is exactly how the service renders that
//! position — the lower-case hex the token's encoded fields are written in, and the
//! fixed-width lower-case hex of its issue instant. Any other spelling of the same
//! bytes (upper-case or mixed-case hex digits, a signed instant, an instant written
//! with more or fewer digits) is a token no listing of this service ever handed out, so
//! it resumes nothing and is refused with the modelled client error — the same rule the
//! service already applies to its other opaque label, a `SchemaVersion`.
//!
//! Asserted over a TABLE, because the reader every listing resumes through is one: the
//! paginated listings of this service — two of them differing only in a selecting
//! member — crossed with every re-spelling, each listing's own genuine rendering serving
//! as the control row that must still be served. The rows differ along which field is
//! re-spelled, which case it is re-spelled into, and how an instant's digits are
//! re-written (a sign, fewer digits, more digits).
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`. The harness is the crate's one harness, from
//! `registry_schema_lifecycle`.

use serde_json::{json, Value};

use crate::discoverer_state::create_discoverer;
use crate::next_token_lifetime::discoverer_ids;
use crate::registry_schema_lifecycle::{
    create_registry, registry_names, schema_names, schema_uri, versions_uri,
    wait_for_keyword_index, walk, Answer, Emulator,
};
use crate::resource_tags::percent_encode;

/// The registry the schema listings below page over, and the prefix its sibling
/// registries share.
const REGISTRY: &str = "token-spellings";

/// The schemas the listings page over. Their names hold characters whose hex rendering
/// carries letters, so a re-cased spelling of a resume key is a different string.
const SCHEMAS: &[&str] = &["orders.cancelled", "orders.placed"];

/// `uri` with one more query member appended, whatever query string it already carries.
fn with_query(uri: &str, member: &str) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{member}")
}

/// The version labels a `ListSchemaVersions` answer reports, in the order it lists them.
fn version_labels(answer: &Answer) -> Vec<String> {
    answer.body["SchemaVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a SchemaVersions array in {:?}", answer.body))
        .iter()
        .map(|item| {
            item["SchemaVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The fields a `NextToken` of this service is rendered from, as the token carries them:
/// the listing's selection, the key the next page resumes after, and the instant the
/// token was issued at.
#[derive(Clone, Debug)]
struct TokenFields {
    selection: String,
    key: String,
    instant: String,
}

impl TokenFields {
    /// The fields of a token this service handed out.
    fn of(case: &str, token: &str) -> Self {
        let fields: Vec<&str> = token.split('.').collect();
        assert_eq!(
            fields.len(),
            3,
            "case {case:?}: expected a token of three fields, got {token:?}"
        );
        Self {
            selection: fields[0].to_string(),
            key: fields[1].to_string(),
            instant: fields[2].to_string(),
        }
    }

    /// The token these fields spell.
    fn render(&self) -> String {
        format!("{}.{}.{}", self.selection, self.key, self.instant)
    }

    /// The same selection and key, with the issue instant replaced.
    fn with_instant(&self, instant: String) -> String {
        Self {
            instant,
            ..self.clone()
        }
        .render()
    }

    /// The seconds the issue instant field encodes.
    fn issued_seconds(&self, case: &str) -> u64 {
        u64::from_str_radix(&self.instant, 16).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the issue instant of a minted token is hex, got {:?} ({error})",
                self.instant
            )
        })
    }
}

/// `field` with every hex digit that has another case upper-cased, or `None` when it
/// holds none: an all-digit field has one spelling only, so there is nothing for a
/// case row to present.
fn upper_cased(field: &str) -> Option<String> {
    let upper = field.to_ascii_uppercase();
    (upper != field).then_some(upper)
}

/// `field` with its FIRST letter upper-cased and every other digit left alone — a
/// mixed-case spelling, which a hex reader takes as readily as either pure case.
fn mixed_case(field: &str) -> Option<String> {
    let at = field.find(|digit: char| digit.is_ascii_alphabetic())?;
    let mut mixed = field.to_string();
    mixed.replace_range(at..at + 1, &field[at..at + 1].to_ascii_uppercase());
    Some(mixed)
}

/// `field` with its leading zeros removed, or `None` when it has none.
fn without_leading_zeros(field: &str) -> Option<String> {
    let trimmed = field.trim_start_matches('0');
    (trimmed != field).then(|| trimmed.to_string())
}

/// One spelling a token is presented in, built from the fields of a token the service
/// really minted.
struct Respelling {
    case: &'static str,
    /// The spelling this row presents, or `None` when the minted rendering already IS
    /// that spelling, so this row has nothing to present for this token.
    respell: fn(&TokenFields) -> Option<String>,
}

/// The table of spellings, none of which this service renders.
const RESPELLINGS: &[Respelling] = &[
    Respelling {
        case: "every hex digit of the whole token upper-cased",
        respell: |fields| upper_cased(&fields.render()),
    },
    Respelling {
        case: "the selection field upper-cased",
        respell: |fields| {
            Some(
                TokenFields {
                    selection: upper_cased(&fields.selection)?,
                    ..fields.clone()
                }
                .render(),
            )
        },
    },
    Respelling {
        case: "one digit of the selection field upper-cased",
        respell: |fields| {
            Some(
                TokenFields {
                    selection: mixed_case(&fields.selection)?,
                    ..fields.clone()
                }
                .render(),
            )
        },
    },
    Respelling {
        case: "the resume key upper-cased",
        respell: |fields| {
            Some(
                TokenFields {
                    key: upper_cased(&fields.key)?,
                    ..fields.clone()
                }
                .render(),
            )
        },
    },
    Respelling {
        case: "one digit of the resume key upper-cased",
        respell: |fields| {
            Some(
                TokenFields {
                    key: mixed_case(&fields.key)?,
                    ..fields.clone()
                }
                .render(),
            )
        },
    },
    Respelling {
        case: "the issue instant upper-cased",
        respell: |fields| Some(fields.with_instant(upper_cased(&fields.instant)?)),
    },
    Respelling {
        case: "the issue instant signed in place of its leading zero",
        respell: |fields| {
            let digits = fields.instant.strip_prefix('0')?;
            Some(fields.with_instant(format!("+{digits}")))
        },
    },
    Respelling {
        case: "the issue instant signed before all of its digits",
        respell: |fields| Some(fields.with_instant(format!("+{}", fields.instant))),
    },
    Respelling {
        case: "the issue instant with its leading zeros stripped",
        respell: |fields| Some(fields.with_instant(without_leading_zeros(&fields.instant)?)),
    },
    Respelling {
        case: "the issue instant with one leading zero dropped",
        respell: |fields| {
            let digits = fields.instant.strip_prefix('0')?;
            Some(fields.with_instant(digits.to_string()))
        },
    },
    Respelling {
        case: "the issue instant with four more leading zeros",
        respell: |fields| Some(fields.with_instant(format!("0000{}", fields.instant))),
    },
];

/// One paginated listing a token is presented to: the route carrying its selection, and
/// how the items of a page of it are read out of the answer.
struct Listing {
    case: &'static str,
    uri: String,
    items: fn(&Answer) -> Vec<String>,
}

/// Create everything the listings below page over, and name them.
///
/// Every listing of this service that pages through the one shared token reader is
/// here, so a spelling honoured by any of them is caught.
async fn seed(emulator: &Emulator) -> Vec<Listing> {
    create_registry(emulator, REGISTRY).await;
    create_registry(emulator, &format!("{REGISTRY}-sibling")).await;
    for schema in SCHEMAS {
        let content = json!({
            "openapi": "3.0.0",
            "info": {"version": "1.0.0", "title": schema},
            "paths": {},
        })
        .to_string();
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, schema),
                Some(json!({"Content": content, "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {schema:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
    }

    // A second version of one schema, so its version listing has a position to resume
    // after as well.
    let second = json!({
        "openapi": "3.0.0",
        "info": {"version": "2.0.0", "title": SCHEMAS[1]},
        "paths": {},
    })
    .to_string();
    let updated = emulator
        .call(
            "PUT",
            &schema_uri(REGISTRY, SCHEMAS[1]),
            Some(json!({"Content": second, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected 200 from UpdateSchema, got {} {:?}",
        updated.status, updated.body
    );

    for bus in ["spellings-one", "spellings-two"] {
        create_discoverer(emulator, "spellings", bus).await;
    }

    // The searches below match on the schema names, but a registry's keyword index is
    // what a search reads, so the cases start once it has caught up.
    wait_for_keyword_index();

    vec![
        Listing {
            case: "ListSchemas",
            uri: format!("/v1/registries/name/{REGISTRY}/schemas"),
            items: schema_names,
        },
        Listing {
            case: "ListSchemas under a name prefix",
            uri: format!("/v1/registries/name/{REGISTRY}/schemas?schemaNamePrefix=orders"),
            items: schema_names,
        },
        Listing {
            case: "ListRegistries",
            uri: format!("/v1/registries?scope=Local&registryNamePrefix={REGISTRY}"),
            items: registry_names,
        },
        Listing {
            case: "ListSchemaVersions",
            uri: versions_uri(REGISTRY, SCHEMAS[1]),
            items: version_labels,
        },
        Listing {
            case: "ListDiscoverers",
            uri: "/v1/discoverers".to_string(),
            items: discoverer_ids,
        },
        Listing {
            case: "SearchSchemas",
            uri: format!("/v1/registries/name/{REGISTRY}/schemas/search?keywords=orders"),
            items: schema_names,
        },
    ]
}

/// Present `token` to `listing` at a page size of one.
async fn present(emulator: &Emulator, listing: &Listing, token: &str) -> Answer {
    emulator
        .call(
            "GET",
            &with_query(
                &listing.uri,
                // The token travels percent-encoded: a raw `+` in a query string is a
                // space, and the cases that present a SIGNED instant must put a sign on
                // the wire.
                &format!("limit=1&nextToken={}", percent_encode(token)),
            ),
            None,
        )
        .await
}

/// RULE: only the rendering this service mints names a position in a listing, so a
/// token re-spelled in any way the minter never renders — a re-cased hex field, a
/// signed issue instant, an instant of another width — resumes nothing and is refused,
/// while the rendering itself is served.
#[tokio::test]
async fn only_the_rendering_a_listing_mints_resumes_it() {
    let emulator = Emulator::new();
    let listings = seed(&emulator).await;

    // How many (listing, token) pairs each row of the table found a spelling to
    // present for: a row that presented nothing anywhere would assert nothing.
    let mut presented = vec![0_usize; RESPELLINGS.len()];

    for listing in &listings {
        let all = walk(&emulator, &listing.uri, None, listing.items).await;
        assert!(
            all.len() >= 2,
            "case {:?}: the listing must hold more than one item to have a page to \
             resume, got {all:?}",
            listing.case
        );
        let expected = vec![all[1].clone()];

        let first = emulator
            .call("GET", &with_query(&listing.uri, "limit=1"), None)
            .await;
        assert_eq!(
            first.status, 200,
            "case {:?}: expected 200 for the first page, got {} {:?}",
            listing.case, first.status, first.body
        );
        let issued = first
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a listing with more to give mints a NextToken, got {:?}",
                    listing.case, first.body
                )
            })
            .to_string();
        let minted = TokenFields::of(listing.case, &issued);

        // The same position, issued a few seconds either side of that instant at a
        // second the service renders with a hex LETTER in it — a rendering of this
        // service just as the issued token is, which gives the case rows a digit of
        // the instant to re-case too.
        let seconds = minted.issued_seconds(listing.case);
        let lettered = TokenFields {
            instant: format!("{:016x}", seconds - seconds % 16 + 10),
            ..minted.clone()
        };
        assert!(
            lettered.instant.contains('a'),
            "case {:?}: the instant re-rendered for the case rows must carry a letter, \
             got {:?}",
            listing.case,
            lettered.instant
        );

        for base in [&minted, &lettered] {
            // The control row: a rendering the service makes IS served, and from the
            // position it names — so every refusal below is about the spelling.
            let rendering = base.render();
            let served = present(&emulator, listing, &rendering).await;
            assert_eq!(
                served.status, 200,
                "case {:?}: the token's own rendering {rendering:?} must resume the \
                 listing, got {} {:?}",
                listing.case, served.status, served.body
            );
            let items = (listing.items)(&served);
            assert_eq!(
                items, expected,
                "case {:?}: the rendering {rendering:?} resumes after the page it was \
                 issued for, expected {expected:?}, got {items:?}",
                listing.case
            );

            for (row, spelling) in RESPELLINGS.iter().enumerate() {
                let Some(respelled) = (spelling.respell)(base) else {
                    continue;
                };
                presented[row] += 1;
                let case = format!("{} / {}", listing.case, spelling.case);
                assert_ne!(
                    respelled, rendering,
                    "case {case:?}: the row must present a spelling the service does \
                     not render, got the rendering itself"
                );
                let answer = present(&emulator, listing, &respelled).await;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: {respelled:?} is no token this service issued, so it \
                     must be refused, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "BadRequestException",
                    "case {case:?}: expected BadRequestException, got {:?}",
                    answer.body
                );
            }
        }
    }

    for (row, spelling) in RESPELLINGS.iter().enumerate() {
        assert!(
            presented[row] > 0,
            "case {:?}: the table presented this spelling to no listing, so the row \
             asserts nothing; got {} presentations",
            spelling.case,
            presented[row]
        );
    }
}
