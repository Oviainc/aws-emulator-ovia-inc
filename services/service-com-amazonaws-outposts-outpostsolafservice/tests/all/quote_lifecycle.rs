// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the AWS Outposts quote lifecycle, driven over the wire
//! through the same front door as [`crate::site_and_outpost_lifecycle`] (whose
//! harness this suite reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `CreateQuote` mints a `QuoteId` of the form its model declares, persists the
//!    quote the request describes, and answers it from what was persisted: every
//!    `Requested…` member is "the … specified in the quote request", so each is the
//!    request's own value, and a member the request named none for is absent rather
//!    than invented.
//! 2. `GetQuote` answers the stored quote field for field, whether the caller named
//!    it by bare id or by the full quote ARN — the two forms the modelled
//!    `@httpLabel` pattern `^(arn:…:quote/)?oq-[a-f0-9]{17}$` accepts — and reading
//!    one quote never changes what a later read of it, or of any other, sees.
//! 3. A quote reports the terminal `EXPIRED` status exactly when the
//!    `ExpirationDate` it itself reports has passed, and the `CREATED` status until
//!    then. The transition is owed at that instant, so the first read at or after it
//!    reports the expiry whether or not anybody called in between.
//! 4. An identifier that names no quote is 404 `NotFoundException`, with the
//!    identifier echo of the value AS SUPPLIED; one that is not a quote identifier
//!    at all is 400 `ValidationException`.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{
    is_minted_id, refusal, segment, text, without, Outposts, Refusal,
};

/// The ARN prefix of a quote of this emulator, from the `QuoteIdentifier` pattern's
/// ARN branch `arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:quote/` plus the home
/// region and the canonical emulator account id.
const QUOTE_ARN_PREFIX: &str = "arn:aws:outposts:us-east-1:000000000000:quote/";

/// The account id every generated emulator presents, which `Quote$AccountId`
/// (`^\d{12}$`) carries.
const ACCOUNT_ID: &str = "000000000000";

/// A quote id this emulator never mints, used wherever a case needs an identifier
/// that is well formed (so the boundary accepts it) but names nothing.
const ABSENT_QUOTE_ID: &str = "oq-00000000000000000";

/// An Outpost id this emulator never mints. See [`ABSENT_QUOTE_ID`].
const ABSENT_OUTPOST_ID: &str = "op-00000000000000000";

/// The members a `Quote` carries only once something has happened to it that this
/// slice of the API cannot make happen: prices come from AWS's own price list, an
/// `OrderingRequirement` is the outcome of a check against the order pipeline, a
/// `SubmittedOrderId` appears when an order is submitted from the quote, and a
/// `StatusMessage` explains a status that needs explaining.
const UNREPORTED: &[&str] = &[
    "QuoteOptions",
    "OrderingRequirements",
    "SubmittedOrderId",
    "StatusMessage",
];

/// The two identifier forms the modelled `QuoteIdentifier` pattern accepts for one
/// quote: the bare id, and the full quote ARN.
fn identifier_forms(quote_id: &str) -> Vec<String> {
    vec![
        quote_id.to_string(),
        format!("{QUOTE_ARN_PREFIX}{quote_id}"),
    ]
}

/// A `CreateQuote` request body: the two members the model marks `@required`, with
/// `overrides` merged in.
pub(crate) fn request(country_code: &str, capacities: Value, overrides: Value) -> Value {
    let mut body = json!({ "CountryCode": country_code, "RequestedCapacities": capacities });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), overrides.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// The instant a timestamp member carries, in epoch seconds.
///
/// restJson1 renders a `@timestamp` with no `@timestampFormat` as epoch seconds;
/// the ISO 8601 rendering is accepted too so the assertion is about the instant and
/// not about which of the modelled renderings the wire used.
fn instant(quote: &Value, member: &str, case: &str) -> f64 {
    match quote.get(member) {
        Some(Value::Number(number)) => number
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {member} is not a finite number")),
        Some(Value::String(rendered)) => DateTime::from_str(rendered, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {member} {rendered:?} is no timestamp: {error}")
            })
            .as_secs_f64(),
        other => panic!("case {case:?}: {member} must be a timestamp, got {other:?}"),
    }
}

/// `CreateQuote`, returning the `Quote` it answered with.
pub(crate) async fn create_quote(outposts: &Outposts, body: &Value) -> Value {
    outposts.ok("POST", "/quotes", Some(body)).await["Quote"].clone()
}

/// `GetQuote` on an identifier, returning the `Quote` it answered with.
async fn get_quote(outposts: &Outposts, identifier: &str) -> Value {
    outposts
        .ok("GET", &format!("/quotes/{}", segment(identifier)), None)
        .await["Quote"]
        .clone()
}

/// One Outpost to associate quotes with, as its `OutpostId` and its `OutpostArn`.
async fn an_outpost(outposts: &Outposts) -> (String, String) {
    outposts.site_and_outpost("quote-outpost").await
}

/// Rules 1 + 2: a `CreateQuote` mints a `QuoteId` of the modelled form, stores what
/// the caller sent, and `GetQuote` answers that same record — by bare id and by ARN
/// alike.
///
/// The rows differ along everything `CreateQuote` generalises over: how many
/// requested capacities (none, one, every modelled capacity type), whether the
/// optional constraints, payment options, payment terms and description are present,
/// how many of each, the country code, and whether the quote is associated with an
/// Outpost — by bare id, by Outpost ARN, or not at all.
#[tokio::test]
async fn create_quote_persists_and_both_identifier_forms_read_it_back() {
    let outposts = Outposts::new();
    let (outpost_id, outpost_arn) = an_outpost(&outposts).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "required members only, empty capacity list",
            request("US", json!([]), json!({})),
        ),
        (
            "one EC2 capacity",
            request(
                "US",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 4.0 }]),
                json!({}),
            ),
        ),
        (
            "every modelled capacity type, with a fractional quantity",
            request(
                "GB",
                json!([
                    { "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 12.0 },
                    { "QuoteCapacityType": "EBS", "Unit": "TB", "Quantity": 2.5 },
                    { "QuoteCapacityType": "S3", "Unit": "TB", "Quantity": 48.0 },
                ]),
                json!({}),
            ),
        ),
        (
            "every modelled constraint type, one payment option, one term",
            request(
                "DE",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 1.0 }]),
                json!({
                    "RequestedConstraints": [
                        { "QuoteConstraintType": "RACK_MAXIMUM", "Value": "2" },
                        { "QuoteConstraintType": "RACK_MAX_POWER_KVA", "Value": "10" },
                        { "QuoteConstraintType": "RACK_MAX_WEIGHT_LBS", "Value": "2000" },
                        { "QuoteConstraintType": "RACK_SPACE_CONSTRAINED", "Value": "true" },
                    ],
                    "RequestedPaymentOptions": ["ALL_UPFRONT"],
                    "RequestedPaymentTerms": ["THREE_YEARS"],
                    "Description": "four constraints",
                }),
            ),
        ),
        (
            "every payment option and every payment term",
            request(
                "JP",
                json!([{ "QuoteCapacityType": "S3", "Unit": "TB", "Quantity": 100.0 }]),
                json!({
                    "RequestedPaymentOptions": ["ALL_UPFRONT", "NO_UPFRONT", "PARTIAL_UPFRONT"],
                    "RequestedPaymentTerms": ["ONE_YEAR", "THREE_YEARS", "FIVE_YEARS"],
                }),
            ),
        ),
        (
            "the empty description the modelled length(min: 0) allows",
            request("FR", json!([]), json!({ "Description": "" })),
        ),
        (
            "a multi-line description",
            request(
                "CA",
                json!([{ "QuoteCapacityType": "EBS", "Unit": "TB", "Quantity": 0.5 }]),
                json!({ "Description": "first line\nsecond line" }),
            ),
        ),
        (
            "associated with an Outpost by bare id",
            request(
                "US",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 8.0 }]),
                json!({ "OutpostIdentifier": outpost_id.clone() }),
            ),
        ),
        (
            "associated with the same Outpost by ARN",
            request(
                "US",
                json!([]),
                json!({ "OutpostIdentifier": outpost_arn.clone() }),
            ),
        ),
    ];

    for (case, body) in &cases {
        let created = create_quote(&outposts, body).await;
        let quote_id = text(&created, "QuoteId");
        assert!(
            is_minted_id(&quote_id, "oq"),
            "case {case:?}: QuoteId {quote_id:?} is not the modelled ^oq-[a-f0-9]{{17}}$ form"
        );
        assert_eq!(
            text(&created, "AccountId"),
            ACCOUNT_ID,
            "case {case:?}: the quote must carry this account's id, got {created}"
        );
        assert_eq!(
            text(&created, "QuoteStatus"),
            "CREATED",
            "case {case:?}: a just-created quote is CREATED, got {created}"
        );
        assert_eq!(
            text(&created, "CountryCode"),
            text(body, "CountryCode"),
            "case {case:?}: the quote must report the country code the request sent, got {created}"
        );

        // Every `Requested…` member is the request's own value, present exactly as
        // sent and absent when the request named none. `RequestedCapacities` is
        // `@required`, so it is always reported — including as the empty list.
        for member in [
            "RequestedCapacities",
            "RequestedConstraints",
            "RequestedPaymentOptions",
            "RequestedPaymentTerms",
            "Description",
        ] {
            assert_eq!(
                created.get(member),
                body.get(member),
                "case {case:?}: {member} must be what the request sent ({:?}), got {:?}",
                body.get(member),
                created.get(member)
            );
        }

        // An Outpost association is reported as that Outpost's OWN ARN, whichever
        // of the two identifier forms the request named it by; a standalone quote
        // reports no Outpost at all.
        let expected_outpost = body
            .get("OutpostIdentifier")
            .map(|_| outpost_arn.clone());
        assert_eq!(
            created.get("OutpostArn").and_then(Value::as_str),
            expected_outpost.as_deref(),
            "case {case:?}: expected OutpostArn {expected_outpost:?}, got {created}"
        );

        for member in UNREPORTED {
            assert!(
                created.get(member).is_none(),
                "case {case:?}: {member} is not something this emulator can compute, so it \
                 must be absent, got {created}"
            );
        }

        let created_date = instant(&created, "CreatedDate", case);
        let expiration_date = instant(&created, "ExpirationDate", case);
        let validity =
            service_com_amazonaws_outposts_outpostsolafservice::state::QUOTE_VALIDITY.as_secs_f64();
        assert!(
            (expiration_date - created_date - validity).abs() < 0.001,
            "case {case:?}: a quote expires {validity} seconds after it was created, got \
             {created_date} -> {expiration_date}"
        );

        // Rule 2: both identifier forms name the same stored record, and the read
        // answers it field for field.
        for identifier in identifier_forms(&quote_id) {
            let read = get_quote(&outposts, &identifier).await;
            assert_eq!(
                read, created,
                "case {case:?}: GetQuote on {identifier:?} must answer the stored quote"
            );
        }
    }
}

/// Rule 2, stated over the whole store: distinct creates mint distinct ids, every
/// quote reads back as ITS OWN record, and repeating a read answers the same thing.
///
/// The rows differ along the members that distinguish two quotes, so a handler that
/// answered the wrong record — or one that mutated a record as it reported it —
/// would show up as a mismatch rather than as a coincidence.
#[tokio::test]
async fn every_persisted_quote_reads_back_as_its_own_record() {
    let outposts = Outposts::new();
    let bodies: Vec<(&str, Value)> = vec![
        ("no capacities", request("US", json!([]), json!({}))),
        (
            "one capacity",
            request(
                "GB",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 2.0 }]),
                json!({ "Description": "second" }),
            ),
        ),
        (
            "two capacities and a constraint",
            request(
                "IE",
                json!([
                    { "QuoteCapacityType": "EBS", "Unit": "TB", "Quantity": 1.0 },
                    { "QuoteCapacityType": "S3", "Unit": "TB", "Quantity": 4.0 },
                ]),
                json!({
                    "RequestedConstraints": [{ "QuoteConstraintType": "RACK_MAXIMUM", "Value": "1" }],
                }),
            ),
        ),
        (
            "every payment term",
            request(
                "AU",
                json!([]),
                json!({ "RequestedPaymentTerms": ["ONE_YEAR", "THREE_YEARS", "FIVE_YEARS"] }),
            ),
        ),
    ];

    let mut created: Vec<(&str, Value)> = Vec::new();
    for (case, body) in &bodies {
        created.push((case, create_quote(&outposts, body).await));
    }

    let ids: std::collections::BTreeSet<String> = created
        .iter()
        .map(|(_, quote)| text(quote, "QuoteId"))
        .collect();
    assert_eq!(
        ids.len(),
        created.len(),
        "every create mints its own QuoteId, got {ids:?}"
    );

    // Every quote is still exactly what its create answered, after every other
    // create and after being read twice: a read reports, it does not change.
    for (case, quote) in &created {
        let identifier = text(quote, "QuoteId");
        for pass in ["first read", "second read"] {
            let read = get_quote(&outposts, &identifier).await;
            assert_eq!(
                &read, quote,
                "case {case:?} ({pass}): GetQuote must answer the quote its create reported"
            );
        }
    }
}

/// Rule 3: a quote is `EXPIRED` exactly when the `ExpirationDate` it reports has
/// passed, and `CREATED` until then.
///
/// The expiry falls a month out, which no test may sleep for, so each row ages the
/// STORED quote by writing the `ExpirationDate` the emulator itself would have
/// reached and then asks over the wire — the transition is owed at that instant, so
/// what the read answers is the whole rule. The rows span both sides of the
/// boundary: an expiry long past, one just past, one just ahead and one a full
/// validity window ahead.
#[tokio::test]
async fn a_quote_is_expired_exactly_when_its_expiration_date_has_passed() {
    let outposts = Outposts::new();
    let validity =
        service_com_amazonaws_outposts_outpostsolafservice::state::QUOTE_VALIDITY.as_secs_f64();
    let cases: Vec<(&str, f64, &str)> = vec![
        ("a month past its expiry", -validity, "EXPIRED"),
        ("a day past its expiry", -86_400.0, "EXPIRED"),
        ("one second past its expiry", -1.0, "EXPIRED"),
        ("a minute before its expiry", 60.0, "CREATED"),
        ("a whole validity window ahead", validity, "CREATED"),
    ];

    for (case, offset, expected) in &cases {
        // A fresh quote per row: the two statuses are reached from `CREATED`, and
        // `EXPIRED` is terminal, so a row must not inherit another row's outcome.
        let created = create_quote(
            &outposts,
            &request(
                "US",
                json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 1.0 }]),
                json!({ "Description": "expiry" }),
            ),
        )
        .await;
        let quote_id = text(&created, "QuoteId");

        let aged = DateTime::from_secs_f64(
            DateTime::from(std::time::SystemTime::now()).as_secs_f64() + offset,
        );
        {
            let store = outposts.store();
            let mut store = store.lock();
            let record = store
                .quotes
                .get_mut(&quote_id)
                .unwrap_or_else(|| panic!("case {case:?}: the create persisted the quote"));
            record.quote.expiration_date = Some(aged);
        }

        let read = get_quote(&outposts, &quote_id).await;
        let status = text(&read, "QuoteStatus");
        assert_eq!(
            status, *expected,
            "case {case:?}: a quote whose ExpirationDate is {offset} seconds from now is \
             {expected}, got {status:?}"
        );
        // The read materialised the expiry and nothing else: every other member is
        // what the create reported (the `ExpirationDate` is the one this row aged).
        assert_eq!(
            without(&without(&read, "QuoteStatus"), "ExpirationDate"),
            without(&without(&created, "QuoteStatus"), "ExpirationDate"),
            "case {case:?}: reading a quote changed a member other than the status it owed"
        );
        // And the status it reported is stable: a later read answers the same.
        let again = get_quote(&outposts, &quote_id).await;
        assert_eq!(
            again, read,
            "case {case:?}: a second GetQuote must answer exactly what the first did"
        );
    }
}

/// Rule 4: an identifier that names no quote is 404 `NotFoundException`; one that is
/// no quote identifier at all is 400 `ValidationException`, from the boundary.
///
/// The rows span both identifier forms, an identifier of ANOTHER resource kind, and
/// the ways a quote id can be malformed (wrong prefix, non-hex suffix, uppercase hex,
/// too few and too many hex characters) — plus the same split on the
/// `OutpostIdentifier` a create may name.
#[tokio::test]
async fn an_absent_quote_is_not_found_and_a_malformed_identifier_is_refused() {
    let outposts = Outposts::new();
    let cases: Vec<Refusal> = vec![
        refusal(
            "GetQuote on a well-formed quote id that names nothing",
            "GET",
            format!("/quotes/{ABSENT_QUOTE_ID}"),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetQuote on the ARN form of a quote that names nothing",
            "GET",
            format!("/quotes/{}", segment(&format!("{QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}"))),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetQuote on a label that is no identifier at all",
            "GET",
            "/quotes/not-a-quote-id".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetQuote on an Outpost id",
            "GET",
            format!("/quotes/{ABSENT_OUTPOST_ID}"),
            400,
            "ValidationException",
        ),
        refusal(
            "GetQuote on a quote id whose suffix is not hex",
            "GET",
            "/quotes/oq-zzzzzzzzzzzzzzzzz".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetQuote on a quote id whose hex is uppercase",
            "GET",
            "/quotes/oq-0000000000000000A".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetQuote on a quote id one hex character short",
            "GET",
            "/quotes/oq-0000000000000000".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetQuote on a quote id one hex character long",
            "GET",
            "/quotes/oq-000000000000000000".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "CreateQuote naming an Outpost that does not exist",
            "POST",
            "/quotes".to_string(),
            404,
            "NotFoundException",
        )
        .with_body(request(
            "US",
            json!([]),
            json!({ "OutpostIdentifier": ABSENT_OUTPOST_ID }),
        )),
        refusal(
            "CreateQuote naming the ARN of an Outpost that does not exist",
            "POST",
            "/quotes".to_string(),
            404,
            "NotFoundException",
        )
        .with_body(request(
            "US",
            json!([]),
            json!({
                "OutpostIdentifier":
                    format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/{ABSENT_OUTPOST_ID}"),
            }),
        )),
    ];

    outposts.expect_refusals(&cases).await;
}

/// Rule 4's message half: a not-found refusal is the identifier ECHO
/// `<member>=<value as supplied>` — the ARN form reads its ARN back, not the bare id
/// the service resolved it to, and the member named is the one the identifier
/// arrived in.
#[tokio::test]
async fn a_not_found_quote_echoes_the_identifier_the_request_supplied() {
    let outposts = Outposts::new();
    let absent_outpost_arn =
        format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/{ABSENT_OUTPOST_ID}");
    let cases: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "GetQuote by bare id",
            "GET",
            format!("/quotes/{ABSENT_QUOTE_ID}"),
            None,
            format!("quoteIdentifier={ABSENT_QUOTE_ID}"),
        ),
        (
            "GetQuote by ARN",
            "GET",
            format!("/quotes/{}", segment(&format!("{QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}"))),
            None,
            format!("quoteIdentifier={QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}"),
        ),
        (
            "CreateQuote naming an absent Outpost by bare id",
            "POST",
            "/quotes".to_string(),
            Some(request(
                "US",
                json!([]),
                json!({ "OutpostIdentifier": ABSENT_OUTPOST_ID }),
            )),
            format!("outpostIdentifier={ABSENT_OUTPOST_ID}"),
        ),
        (
            "CreateQuote naming an absent Outpost by ARN",
            "POST",
            "/quotes".to_string(),
            Some(request(
                "US",
                json!([]),
                json!({ "OutpostIdentifier": absent_outpost_arn.clone() }),
            )),
            format!("outpostIdentifier={absent_outpost_arn}"),
        ),
    ];

    for (case, method, path, body, expected) in &cases {
        let reply = outposts.call(method, path, body.as_ref()).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = reply
            .body
            .get("Message")
            .or_else(|| reply.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the identifier echo {expected:?}, got {actual:?}"
        );
    }
}
