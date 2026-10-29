// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the bound every refusal body this service words keeps,
//! driven over the wire through `MultiServiceRouter::dispatch` exactly as a signed
//! `aws outposts` call arrives (reusing the harness of
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: every
//! failure shape in this API carries its message as `ErrorMessage`,
//! `length(min: 1, max: 1000)` with pattern `^[\S \n]+$`
//! (`smithy_query(query="shape",shape="ErrorMessage")`), so no refusal body this
//! service words is longer than that — whatever a request carried. Where a body
//! renders a value the caller supplied, the ECHO of that value is what takes the
//! cut, so the refusal's own fixed wording survives for any value that reaches a
//! handler.
//!
//! That matters because several members a refusal echoes are modelled LONGER than
//! the message reporting them: `GetConnection$ConnectionId` is `length(1..1024)`
//! and the `Arn` every tagging operation binds is `length(0..1011)`
//! (`smithy_query(query="operation",operation="GetConnection,ListTagsForResource")`),
//! while `CreateOutpost$AvailabilityZone` is `length(1..1000)` — so a value the
//! boundary validator accepts can be longer than the body that refuses it.
//!
//! What the body SAYS for an identifier that already fits is
//! [`crate::error_message_rules`]' rule A, which these rows leave exactly as it is:
//! such an identifier is echoed whole.

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{segment, Outposts, ABSENT_SITE_ID, HOME_REGION};

/// The longest message the model accepts in any failure of this API
/// (`ErrorMessage`, `length(min: 1, max: 1000)`).
const ERROR_MESSAGE_MAX: usize = 1000;

/// The shortest one it accepts: a refusal always carries a body.
const ERROR_MESSAGE_MIN: usize = 1;

/// The modelled `ConnectionId` maximum length (`length(1..1024)`, pattern
/// `^[a-zA-Z0-9+/=]{1,1024}$`).
const CONNECTION_ID_MAX: usize = 1024;

/// The modelled `ResourceArn` maximum length (`Arn`, `length(0..1011)`).
const RESOURCE_ARN_MAX: usize = 1011;

/// The modelled `AvailabilityZone` maximum length (`length(1..1000)`, pattern
/// `^([a-zA-Z]+-){1,3}([a-zA-Z]+)?(\d+[a-zA-Z]?)?$`).
const AVAILABILITY_ZONE_MAX: usize = 1000;

/// The validation-path names the identifier echo spells, lowerCamel as a report
/// names a member (`ConnectionId` -> `connectionId`).
const CONNECTION_ID_MEMBER: &str = "connectionId";
/// See [`CONNECTION_ID_MEMBER`].
const RESOURCE_ARN_MEMBER: &str = "resourceArn";
/// See [`CONNECTION_ID_MEMBER`].
const SITE_ID_MEMBER: &str = "siteId";

/// The account id every generated emulator presents, which its ARNs carry.
const ACCOUNT_ID: &str = "000000000000";

/// A `ConnectionId` of exactly `length` characters drawn from the modelled
/// alphabet, built by repeating `filler` — a value the modelled length and pattern
/// both accept, so it reaches the handler and names no connection any store holds.
fn connection_id(filler: &str, length: usize) -> String {
    filler.chars().cycle().take(length).collect()
}

/// The shortest Outposts resource ARN that names nothing this store holds: this
/// endpoint's own region, and a site id this emulator never mints.
fn short_arn() -> String {
    format!("arn:aws:outposts:{HOME_REGION}:{ACCOUNT_ID}:site/os-00000000000000000")
}

/// An Outposts resource ARN of exactly `length` characters that names nothing this
/// store holds.
///
/// The modelled `Arn` pattern
/// `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$`
/// leaves the REGION field unbounded (`[a-z\d-]+`), which is where the padding goes:
/// the value stays an ARN the boundary accepts, naming a region this endpoint does
/// not serve.
fn padded_arn(length: usize) -> String {
    let padding = length
        .checked_sub(short_arn().len() - HOME_REGION.len())
        .unwrap_or_else(|| panic!("an ARN of {length} characters holds the modelled fixed parts"));
    let arn = format!(
        "arn:aws:outposts:{}:{ACCOUNT_ID}:site/os-00000000000000000",
        "a".repeat(padding)
    );
    assert_eq!(
        arn.len(),
        length,
        "the padded ARN must be {length} characters, got {}",
        arn.len()
    );
    arn
}

/// An `AvailabilityZone` of exactly `length` characters that the modelled pattern
/// accepts — `<letters>-<tail>` — where `tail` is what decides whether the value
/// names a zone at all (a trailing zone letter does, a trailing digit does not).
fn padded_zone(length: usize, tail: &str) -> String {
    let padding = length
        .checked_sub(tail.len() + 1)
        .unwrap_or_else(|| panic!("a zone of {length} characters holds its tail"));
    let zone = format!("{}-{tail}", "a".repeat(padding));
    assert_eq!(
        zone.len(),
        length,
        "the padded zone must be {length} characters, got {}",
        zone.len()
    );
    zone
}

/// The identifier echo this service words a not-found with, cut to fit: `<member>=`
/// and the supplied value as far as the room that prefix leaves inside
/// [`ERROR_MESSAGE_MAX`].
///
/// Every value a row sends is ASCII — the modelled alphabets of these members are —
/// so a byte prefix is a character prefix.
fn expected_echo(member: &str, supplied: &str) -> String {
    let room = ERROR_MESSAGE_MAX - member.len() - "=".len();
    format!("{member}={}", &supplied[..supplied.len().min(room)])
}

/// Whatever a body says, it has to be a value the modelled `ErrorMessage` holds:
/// within its two length bounds, and over its pattern's character class
/// (`^[\S \n]+$` — no other whitespace).
fn assert_within_model(case: &str, message: &str) {
    let length = message.chars().count();
    assert!(
        (ERROR_MESSAGE_MIN..=ERROR_MESSAGE_MAX).contains(&length),
        "case {case:?}: the body must be {ERROR_MESSAGE_MIN}..={ERROR_MESSAGE_MAX} characters \
         to fit the modelled ErrorMessage, got {length}"
    );
    let stray = message
        .chars()
        .find(|character| character.is_whitespace() && *character != ' ' && *character != '\n');
    assert!(
        stray.is_none(),
        "case {case:?}: the body must hold only characters the modelled ErrorMessage pattern \
         admits, got {stray:?}"
    );
}

/// One row: a call that names a resource no store holds, and the input member plus
/// the value it named it with.
struct Echo {
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    member: &'static str,
    supplied: String,
}

/// A row whose identifier travels in the URI, with an optional payload.
fn echo(
    case: String,
    method: &'static str,
    collection: &str,
    query: &str,
    member: &'static str,
    supplied: String,
    body: Option<Value>,
) -> Echo {
    Echo {
        case,
        method,
        path: format!("/{collection}/{}{query}", segment(&supplied)),
        body,
        member,
        supplied,
    }
}

/// The rule over the two members this API models LONGER than the message that
/// reports them, across every operation that binds one.
///
/// The rows differ along the one thing the rule generalises over — how long the
/// supplied value is: the modelled minimum, a middling value, the longest value that
/// still fits the message whole, one character past it, and the member's modelled
/// maximum. They differ too in the alphabet the value is drawn from, and in which
/// operation (and HTTP method) it arrives at.
#[tokio::test]
async fn a_not_found_echo_is_cut_to_the_modelled_message_bound() {
    let outposts = Outposts::new();
    // The room `connectionId=` / `resourceArn=` leave the value inside the bound:
    // the longest echo that is still the whole value.
    let connection_room = ERROR_MESSAGE_MAX - CONNECTION_ID_MEMBER.len() - 1;
    let arn_room = ERROR_MESSAGE_MAX - RESOURCE_ARN_MEMBER.len() - 1;
    let tags = json!({ "Tags": { "bound": "case" } });

    // An identifier that already fits is echoed whole: the cut changes nothing
    // about a value within the bound.
    let mut cases: Vec<Echo> = vec![echo(
        "GetSite on an id nothing holds".to_string(),
        "GET",
        "sites",
        "",
        SITE_ID_MEMBER,
        ABSENT_SITE_ID.to_string(),
        None,
    )];

    for (filler, length) in [
        ("A", 1),
        ("+/=Z9", 44),
        ("a", connection_room - 1),
        ("b", connection_room),
        ("c", connection_room + 1),
        ("=", CONNECTION_ID_MAX - 1),
        ("9zZ+/", CONNECTION_ID_MAX),
    ] {
        cases.push(echo(
            format!("GetConnection on a {length}-character ConnectionId nothing holds"),
            "GET",
            "connections",
            "",
            CONNECTION_ID_MEMBER,
            connection_id(filler, length),
            None,
        ));
    }

    // The three tagging operations bind the SAME `ResourceArn`, so every ARN row
    // goes to all of them: one rule, worded once, whatever the method.
    for arn in [
        short_arn(),
        padded_arn(arn_room - 1),
        padded_arn(arn_room),
        padded_arn(arn_room + 1),
        padded_arn(RESOURCE_ARN_MAX),
    ] {
        for (operation, method, query, body) in [
            ("ListTagsForResource", "GET", "", None),
            ("TagResource", "POST", "", Some(tags.clone())),
            ("UntagResource", "DELETE", "?tagKeys=bound", None),
        ] {
            cases.push(echo(
                format!(
                    "{operation} on a {}-character ResourceArn nothing holds",
                    arn.len()
                ),
                method,
                "tags",
                query,
                RESOURCE_ARN_MEMBER,
                arn.clone(),
                body,
            ));
        }
    }

    for row in &cases {
        let case = &row.case;
        let reply = outposts.call(row.method, &row.path, row.body.as_ref()).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body);
        let expected = expected_echo(row.member, &row.supplied);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected $.Message of {} characters, got {} characters",
            expected.len(),
            actual.len()
        );
        assert_within_model(case, &actual);
        // Whatever was cut, what the body carries is the value AS SUPPLIED as far as
        // it fits — never a normalised or re-spelled identifier.
        let echoed = actual
            .split_once('=')
            .map_or(String::new(), |(_, echoed)| echoed.to_string());
        assert!(
            !echoed.is_empty() && row.supplied.starts_with(&echoed),
            "case {case:?}: the echo must be the supplied value as far as it fits, got {} \
             characters",
            echoed.len()
        );
    }
}

/// One row of the other half: a request refused with a sentence of this service's
/// own that renders a value the caller supplied.
struct Worded {
    case: String,
    body: Value,
    /// What the sentence puts straight AFTER the echo of the supplied value, which
    /// is where the echo stops.
    after_echo: &'static str,
    /// Wording the body must still carry whole, however long the value was.
    wording: &'static str,
}

/// The rule over the refusal that is NOT an identifier echo: the Availability Zone a
/// `CreateOutpost` names, whose modelled member reaches the message bound on its own.
///
/// The rows differ along the three sentences this refusal has — a zone of another
/// region, a value that names no zone of this one, two identities naming two zones —
/// and, for each sentence, along the length of what the caller supplied: a short
/// value and one at the member's modelled maximum.
#[tokio::test]
async fn a_worded_refusal_keeps_its_wording_inside_the_modelled_message_bound() {
    let outposts = Outposts::new();

    // A zone identifier that names a zone (its last character is a zone letter) but
    // whose region part is not this request's, and one that names no zone at all (it
    // ends in a digit), each at the member's modelled maximum length.
    let foreign_zone = padded_zone(AVAILABILITY_ZONE_MAX, "1a");
    let zoneless = padded_zone(AVAILABILITY_ZONE_MAX, "11");

    let cases: Vec<Worded> = vec![
        Worded {
            case: "a short zone of another region".to_string(),
            body: zone_request(json!({ "AvailabilityZone": "us-west-2a" })),
            after_echo: " does not match your region.",
            wording: " does not match your region.",
        },
        Worded {
            case: format!("a {AVAILABILITY_ZONE_MAX}-character zone of another region"),
            body: zone_request(json!({ "AvailabilityZone": foreign_zone })),
            after_echo: " does not match your region.",
            wording: " does not match your region.",
        },
        Worded {
            case: "a short value that names no zone of this region".to_string(),
            body: zone_request(json!({ "AvailabilityZone": HOME_REGION })),
            after_echo: " is not an Availability Zone of ",
            wording: " is not an Availability Zone of ",
        },
        Worded {
            case: format!("a {AVAILABILITY_ZONE_MAX}-character value that names no zone"),
            body: zone_request(json!({ "AvailabilityZone": zoneless })),
            after_echo: " is not an Availability Zone of ",
            wording: " is not an Availability Zone of ",
        },
        Worded {
            case: "two identities that name two zones".to_string(),
            body: zone_request(json!({
                "AvailabilityZone": "us-east-1a",
                "AvailabilityZoneId": "use1-az2",
            })),
            after_echo: " and ",
            wording: " do not name the same Availability Zone.",
        },
    ];

    for row in &cases {
        let case = &row.case;
        let reply = outposts.call("POST", "/outposts", Some(&row.body)).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = message(&reply.body);
        assert_within_model(case, &actual);
        assert!(
            actual.contains(row.wording),
            "case {case:?}: the refusal must still carry its wording {:?}, got {} characters",
            row.wording,
            actual.len()
        );
        // The value is rendered as supplied, as far as the wording leaves room for
        // it: what the body opens with is a prefix of what the request sent.
        let supplied = row.body["AvailabilityZone"].as_str().unwrap_or_default();
        let rendered = actual
            .split_once(row.after_echo)
            .map_or("", |(rendered, _)| rendered);
        assert!(
            !rendered.is_empty() && supplied.starts_with(rendered),
            "case {case:?}: the refusal must render the supplied value as far as it fits, got \
             {} characters",
            rendered.len()
        );
    }
}

/// A `CreateOutpost` request carrying the two members the model marks `@required`
/// plus `overrides` — the Availability Zone identities a row is about. The site is
/// one no store holds, which is enough: the zone is read BEFORE anything the request
/// references is looked up.
fn zone_request(overrides: Value) -> Value {
    let mut request = json!({ "Name": "message bound", "SiteId": ABSENT_SITE_ID });
    if let (Some(target), Some(extra)) = (request.as_object_mut(), overrides.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    request
}

/// The error message a reply carries, under the key its envelope spells it with
/// (`Message` for a modelled error shape, `message` for the framework's synthetic
/// one), read as the empty string when it carries none.
fn message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .find_map(|key| body.get(key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}
