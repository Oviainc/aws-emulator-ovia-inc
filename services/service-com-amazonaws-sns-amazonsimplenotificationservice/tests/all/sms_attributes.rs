// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account-level SMS settings pair, `SetSMSAttributes` and
//! `GetSMSAttributes`, driven over the wire.
//!
//! The rules under test:
//!
//! * a write carries the settings it changes, so the account keeps every setting the
//!   call does not name, and each named setting takes the value the call carried;
//! * a write of an EMPTY value clears the setting, leaving the account as it was before
//!   it was ever given that setting;
//! * a read REPORTS what the account holds — the settings it was given, plus the spend
//!   limit SNS applies to an account that has not been given one — and reports exactly
//!   the settings the request names when it names any, so a read changes nothing;
//! * a name outside the documented set, and a value outside what the named setting
//!   documents, are refused with `400 InvalidParameter` and the whole request then
//!   changes nothing.
//!
//! The rows below differ in which setting is written, the value's form (an amount, a
//! whole number, a sender id, an ARN, an enumerated word), how many settings one call
//! carries, how many names a read filters on, and which documented bound a refused
//! value sits past — which is what the rules generalise over. Every call goes over the
//! wire through the router the emulator serves in production, so nothing but persisted
//! state carries information between calls.

use std::collections::HashMap;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    DEFAULT_MONTHLY_SPEND_LIMIT, DEFAULT_SENDER_ID_ATTRIBUTE, DEFAULT_SMS_TYPES,
    DEFAULT_SMS_TYPE_ATTRIBUTE, DELIVERY_STATUS_IAM_ROLE_ATTRIBUTE,
    DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE, MAX_SAMPLING_RATE, MAX_SENDER_ID_CHARS,
    MIN_SAMPLING_RATE, MIN_SENDER_ID_CHARS, MONTHLY_SPEND_LIMIT_ATTRIBUTE, SMS_ATTRIBUTE_NAMES,
    USAGE_REPORT_S3_BUCKET_ATTRIBUTE,
};

use super::platform_applications::entries_of;
use super::topics_subscriptions_publish::{Answer, Sns};

/// Write `settings` through `SetSMSAttributes`, answering the call as it came back.
///
/// The awsQuery form of an attribute map is one numbered `entry` per setting, which is
/// how every attribute-map write in this suite is spelled.
async fn set(sns: &Sns, settings: &[(&str, &str)]) -> Answer {
    let mut params: Vec<(String, String)> = Vec::new();
    for (index, (name, value)) in settings.iter().enumerate() {
        params.push((
            format!("attributes.entry.{}.key", index + 1),
            (*name).to_string(),
        ));
        params.push((
            format!("attributes.entry.{}.value", index + 1),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    sns.call("SetSMSAttributes", &borrowed).await
}

/// Write `settings` and assert the call was answered.
async fn set_ok(sns: &Sns, case: &str, settings: &[(&str, &str)]) {
    let answer = set(sns, settings).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: SetSMSAttributes {settings:?} expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// The settings a `GetSMSAttributes` naming `requested` reports — every setting the
/// account holds when `requested` is empty.
async fn read(sns: &Sns, case: &str, requested: &[&str]) -> HashMap<String, String> {
    let params: Vec<(String, String)> = requested
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                format!("attributes.member.{}", index + 1),
                (*name).to_string(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let answer = sns.call("GetSMSAttributes", &borrowed).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetSMSAttributes {requested:?} expected 200, got {} / {}",
        answer.status, answer.body
    );
    entries_of(&answer.body)
}

/// A value each documented setting accepts, one row per name in
/// [`SMS_ATTRIBUTE_NAMES`], the rows differing in the FORM the value takes: an amount,
/// an ARN, a whole number, a sender id and a bucket name.
fn accepted_settings() -> Vec<(&'static str, &'static str)> {
    vec![
        (MONTHLY_SPEND_LIMIT_ATTRIBUTE, "25"),
        (
            DELIVERY_STATUS_IAM_ROLE_ATTRIBUTE,
            "arn:aws:iam::000000000000:role/SNSSMSLogs",
        ),
        (DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE, "10"),
        (DEFAULT_SENDER_ID_ATTRIBUTE, "Brand7"),
        (DEFAULT_SMS_TYPE_ATTRIBUTE, "Transactional"),
        (USAGE_REPORT_S3_BUCKET_ATTRIBUTE, "sms-usage-reports"),
    ]
}

/// Every setting [`SMS_ATTRIBUTE_NAMES`] names is written, read back with its own
/// value, and still reported beside the ones written after it: a write carries the
/// settings it names and leaves the rest of the account's record alone.
#[tokio::test]
async fn every_documented_setting_round_trips_and_a_write_keeps_the_others() {
    let sns = Sns::new();

    // The table covers the whole documented name set, so a name this suite did not
    // think of still has to round trip.
    let settings = accepted_settings();
    let covered: Vec<&str> = settings.iter().map(|(name, _)| *name).collect();
    let mut sorted = covered.clone();
    sorted.sort_unstable();
    let mut documented: Vec<&str> = SMS_ATTRIBUTE_NAMES.to_vec();
    documented.sort_unstable();
    assert_eq!(
        sorted, documented,
        "the table must cover every documented SMS setting, got {sorted:?}"
    );

    let mut written: Vec<(&str, &str)> = Vec::new();
    for (name, value) in &settings {
        let case = format!("{name} = {value}");
        set_ok(&sns, &case, &[(*name, *value)]).await;
        written.push((name, value));

        let reported = read(&sns, &case, &[]).await;
        for (held, expected) in &written {
            assert_eq!(
                reported.get(*held).map(String::as_str),
                Some(*expected),
                "case {case:?}: expected {held} to report {expected:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.len(),
            written.len(),
            "case {case:?}: expected exactly the settings written so far, got {reported:?}"
        );
    }

    // A second write of one setting replaces that setting's value and no other.
    let case = "a second write of one setting";
    set_ok(&sns, case, &[(DEFAULT_SMS_TYPE_ATTRIBUTE, "Promotional")]).await;
    let reported = read(&sns, case, &[]).await;
    assert_eq!(
        reported.get(DEFAULT_SMS_TYPE_ATTRIBUTE).map(String::as_str),
        Some("Promotional"),
        "case {case:?}: expected the new value, got {reported:?}"
    );
    assert_eq!(
        reported.get(DEFAULT_SENDER_ID_ATTRIBUTE).map(String::as_str),
        Some("Brand7"),
        "case {case:?}: expected the untouched setting to survive, got {reported:?}"
    );
}

/// One call may carry several settings at once, and each lands with its own value.
#[tokio::test]
async fn one_call_writes_several_settings_at_once() {
    let settings = accepted_settings();
    // The rows differ in how many settings one call carries: one, two, and every
    // documented setting in one request.
    for count in [1usize, 2, settings.len()] {
        let sns = Sns::new();
        let case = format!("{count} setting(s) in one call");
        let batch: Vec<(&str, &str)> = settings.iter().take(count).copied().collect();
        set_ok(&sns, &case, &batch).await;

        let reported = read(&sns, &case, &[]).await;
        for (name, value) in &batch {
            assert_eq!(
                reported.get(*name).map(String::as_str),
                Some(*value),
                "case {case:?}: expected {name} to report {value:?}, got {reported:?}"
            );
        }
    }
}

/// A read that names settings reports EXACTLY those of them the account holds — no
/// sibling setting, one entry for a name asked for twice, and no entry for a name the
/// account holds nothing for.
#[tokio::test]
async fn a_filtered_read_reports_exactly_the_settings_it_names() {
    let sns = Sns::new();
    let held = [
        (DEFAULT_SMS_TYPE_ATTRIBUTE, "Transactional"),
        (DEFAULT_SENDER_ID_ATTRIBUTE, "Brand7"),
        (MONTHLY_SPEND_LIMIT_ATTRIBUTE, "25"),
    ];
    set_ok(&sns, "the settings the filters select from", &held).await;

    // (case, the names the read asks for, the names it must report)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "no filter at all",
            vec![],
            vec![
                DEFAULT_SMS_TYPE_ATTRIBUTE,
                DEFAULT_SENDER_ID_ATTRIBUTE,
                MONTHLY_SPEND_LIMIT_ATTRIBUTE,
            ],
        ),
        (
            "one name",
            vec![DEFAULT_SMS_TYPE_ATTRIBUTE],
            vec![DEFAULT_SMS_TYPE_ATTRIBUTE],
        ),
        (
            "two names",
            vec![DEFAULT_SMS_TYPE_ATTRIBUTE, MONTHLY_SPEND_LIMIT_ATTRIBUTE],
            vec![DEFAULT_SMS_TYPE_ATTRIBUTE, MONTHLY_SPEND_LIMIT_ATTRIBUTE],
        ),
        (
            "one name twice",
            vec![DEFAULT_SENDER_ID_ATTRIBUTE, DEFAULT_SENDER_ID_ATTRIBUTE],
            vec![DEFAULT_SENDER_ID_ATTRIBUTE],
        ),
        (
            "a setting the account holds nothing for",
            vec![USAGE_REPORT_S3_BUCKET_ATTRIBUTE],
            vec![],
        ),
        (
            "a held name beside one the account holds nothing for",
            vec![
                USAGE_REPORT_S3_BUCKET_ATTRIBUTE,
                DEFAULT_SENDER_ID_ATTRIBUTE,
            ],
            vec![DEFAULT_SENDER_ID_ATTRIBUTE],
        ),
    ];

    for (case, requested, expected) in &cases {
        let reported = read(&sns, case, requested).await;
        let mut names: Vec<&str> = reported.keys().map(String::as_str).collect();
        names.sort_unstable();
        let mut wanted: Vec<&str> = expected.clone();
        wanted.sort_unstable();
        assert_eq!(
            names, wanted,
            "case {case:?}: expected exactly {wanted:?}, got {reported:?}"
        );
        for name in expected {
            let value = held
                .iter()
                .find(|(held_name, _)| held_name == name)
                .map(|(_, value)| *value);
            assert_eq!(
                reported.get(*name).map(String::as_str),
                value,
                "case {case:?}: expected {name} to report the value the account holds, \
                 got {reported:?}"
            );
        }
    }
}

/// A write of an empty value CLEARS the setting: the account reports no entry for it
/// afterwards, whichever setting it was, and the settings beside it are untouched.
#[tokio::test]
async fn a_write_of_an_empty_value_clears_the_setting() {
    for (name, value) in accepted_settings() {
        let sns = Sns::new();
        let case = format!("clearing {name}");
        // A sibling setting, so what the clear leaves behind is visible.
        let sibling = if name == DEFAULT_SMS_TYPE_ATTRIBUTE {
            (DEFAULT_SENDER_ID_ATTRIBUTE, "Brand7")
        } else {
            (DEFAULT_SMS_TYPE_ATTRIBUTE, "Transactional")
        };
        set_ok(&sns, &case, &[(name, value), sibling]).await;
        let reported = read(&sns, &case, &[]).await;
        assert_eq!(
            reported.get(name).map(String::as_str),
            Some(value),
            "case {case:?}: the setting must be held before it is cleared, got {reported:?}"
        );

        set_ok(&sns, &case, &[(name, "")]).await;
        let reported = read(&sns, &case, &[]).await;
        if name == MONTHLY_SPEND_LIMIT_ATTRIBUTE {
            // A spend limit is a setting an account always has, so clearing one returns
            // it to the limit SNS applies to an account that has not set one.
            assert_eq!(
                reported.get(name).map(String::as_str),
                Some(DEFAULT_MONTHLY_SPEND_LIMIT),
                "case {case:?}: expected the default spend limit again, got {reported:?}"
            );
        } else {
            assert!(
                !reported.contains_key(name),
                "case {case:?}: expected no entry for the cleared setting, got {reported:?}"
            );
        }
        assert_eq!(
            reported.get(sibling.0).map(String::as_str),
            Some(sibling.1),
            "case {case:?}: expected the sibling setting to survive the clear, got {reported:?}"
        );

        // A filtered read of the cleared setting reports it the same way.
        let filtered = read(&sns, &case, &[name]).await;
        assert_eq!(
            filtered.get(name).map(String::as_str),
            reported.get(name).map(String::as_str),
            "case {case:?}: a filtered read must report the cleared setting as the \
             unfiltered one does, got {filtered:?}"
        );
    }
}

/// An account nobody has written to reports the spend limit SNS applies by default and
/// no other setting, filtered or not.
#[tokio::test]
async fn an_account_reports_the_default_spend_limit_and_nothing_else() {
    let sns = Sns::new();

    for read_number in 0..3 {
        let case = format!("read {read_number} of a fresh account");
        let reported = read(&sns, &case, &[]).await;
        assert_eq!(
            reported.get(MONTHLY_SPEND_LIMIT_ATTRIBUTE).map(String::as_str),
            Some(DEFAULT_MONTHLY_SPEND_LIMIT),
            "case {case:?}: expected the documented default spend limit, got {reported:?}"
        );
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected no setting the account was never given, got {reported:?}"
        );

        let filtered = read(&sns, &case, &[MONTHLY_SPEND_LIMIT_ATTRIBUTE]).await;
        assert_eq!(
            filtered.get(MONTHLY_SPEND_LIMIT_ATTRIBUTE).map(String::as_str),
            Some(DEFAULT_MONTHLY_SPEND_LIMIT),
            "case {case:?}: a filtered read must report it too, got {filtered:?}"
        );

        // The read reports; it does not write. Nothing a read does may leave a setting
        // behind in the record a later caller sees.
        let state = sns.state();
        let held = state.lock().sms_settings.attributes.clone();
        assert!(
            held.is_empty(),
            "case {case:?}: a read must not write a setting, got {held:?}"
        );
    }
}

/// A name outside the documented set is refused, and the refusal names the name the
/// caller sent.
#[tokio::test]
async fn a_name_that_is_no_sms_setting_is_refused() {
    let sns = Sns::new();

    // The rows differ in HOW the name misses the documented set: a word that is no
    // setting at all, a setting of another SNS record, the right name in the wrong
    // case, and a name carrying one of SNS's own spellings as a prefix.
    let cases = [
        "NotARealSetting",
        "DisplayName",
        "defaultsmstype",
        "DefaultSMSTypeExtra",
    ];

    for name in cases {
        let answer = set(&sns, &[(name, "Transactional")]).await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {name:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message"),
            Some(format!("{name} is not a valid attribute")),
            "case {name:?}: expected the refusal to name the attribute, got {}",
            answer.body
        );
    }
}

/// A value outside what the named setting documents is refused, naming that setting —
/// and the value each documented bound admits is accepted.
#[tokio::test]
async fn a_value_the_setting_does_not_document_is_refused() {
    // (case, setting, value, whether the setting accepts it)
    //
    // The documented ranges are tested on each published bound and one past it: the
    // sampling rate is "an integer from 0 - 100" and a sender id "1 - 11 alphanumeric
    // characters … at least one letter"
    // (`smithy_query(query="doc", shape="SetSMSAttributesInput", member="attributes")`).
    let sender_id_at_bound = "A".repeat(MAX_SENDER_ID_CHARS);
    let sender_id_past_bound = "A".repeat(MAX_SENDER_ID_CHARS + 1);
    let shortest_sender_id = "A".repeat(MIN_SENDER_ID_CHARS);
    let lowest_rate = MIN_SAMPLING_RATE.to_string();
    let highest_rate = MAX_SAMPLING_RATE.to_string();
    let past_highest_rate = (MAX_SAMPLING_RATE + 1).to_string();
    let cases: Vec<(String, &str, String, bool)> = vec![
        (
            "the first documented message type".to_string(),
            DEFAULT_SMS_TYPE_ATTRIBUTE,
            DEFAULT_SMS_TYPES[0].to_string(),
            true,
        ),
        (
            "the second documented message type".to_string(),
            DEFAULT_SMS_TYPE_ATTRIBUTE,
            DEFAULT_SMS_TYPES[1].to_string(),
            true,
        ),
        (
            "a message type SNS does not document".to_string(),
            DEFAULT_SMS_TYPE_ATTRIBUTE,
            "Marketing".to_string(),
            false,
        ),
        (
            "a documented message type in the wrong case".to_string(),
            DEFAULT_SMS_TYPE_ATTRIBUTE,
            "transactional".to_string(),
            false,
        ),
        (
            "the lowest documented sampling rate".to_string(),
            DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE,
            lowest_rate,
            true,
        ),
        (
            "the highest documented sampling rate".to_string(),
            DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE,
            highest_rate,
            true,
        ),
        (
            "one past the highest documented sampling rate".to_string(),
            DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE,
            past_highest_rate,
            false,
        ),
        (
            "a sampling rate that is not a whole number".to_string(),
            DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE,
            "12.5".to_string(),
            false,
        ),
        (
            "a negative sampling rate".to_string(),
            DELIVERY_STATUS_SUCCESS_SAMPLING_RATE_ATTRIBUTE,
            "-1".to_string(),
            false,
        ),
        (
            "the shortest documented sender id".to_string(),
            DEFAULT_SENDER_ID_ATTRIBUTE,
            shortest_sender_id,
            true,
        ),
        (
            "the longest documented sender id".to_string(),
            DEFAULT_SENDER_ID_ATTRIBUTE,
            sender_id_at_bound,
            true,
        ),
        (
            "one character past the longest sender id".to_string(),
            DEFAULT_SENDER_ID_ATTRIBUTE,
            sender_id_past_bound,
            false,
        ),
        (
            "a sender id carrying no letter".to_string(),
            DEFAULT_SENDER_ID_ATTRIBUTE,
            "12345".to_string(),
            false,
        ),
        (
            "a sender id carrying a character outside the alphanumerics".to_string(),
            DEFAULT_SENDER_ID_ATTRIBUTE,
            "Brand 7".to_string(),
            false,
        ),
        (
            "a whole spend limit".to_string(),
            MONTHLY_SPEND_LIMIT_ATTRIBUTE,
            "100".to_string(),
            true,
        ),
        (
            "a spend limit carrying cents".to_string(),
            MONTHLY_SPEND_LIMIT_ATTRIBUTE,
            "100.50".to_string(),
            true,
        ),
        (
            "a spend limit that is not an amount".to_string(),
            MONTHLY_SPEND_LIMIT_ATTRIBUTE,
            "a lot".to_string(),
            false,
        ),
        (
            "a negative spend limit".to_string(),
            MONTHLY_SPEND_LIMIT_ATTRIBUTE,
            "-5".to_string(),
            false,
        ),
        (
            "a delivery status role named by its ARN".to_string(),
            DELIVERY_STATUS_IAM_ROLE_ATTRIBUTE,
            "arn:aws:iam::000000000000:role/SNSSMSLogs".to_string(),
            true,
        ),
        (
            "a delivery status role that is not named by an ARN".to_string(),
            DELIVERY_STATUS_IAM_ROLE_ATTRIBUTE,
            "SNSSMSLogs".to_string(),
            false,
        ),
        (
            "a usage report bucket, which SNS constrains no further".to_string(),
            USAGE_REPORT_S3_BUCKET_ATTRIBUTE,
            "sms-usage-reports".to_string(),
            true,
        ),
    ];

    for (case, name, value, accepted) in &cases {
        let sns = Sns::new();
        let answer = set(&sns, &[(name, value)]).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected {name} to accept {value:?}, got {} / {}",
                answer.status, answer.body
            );
            let reported = read(&sns, case, &[name]).await;
            assert_eq!(
                reported.get(*name).map(String::as_str),
                Some(value.as_str()),
                "case {case:?}: expected the accepted value to round trip, got {reported:?}"
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected {name} to refuse {value:?}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.element("Message"),
            Some(format!("{name} is invalid")),
            "case {case:?}: expected the refusal to name the setting, got {}",
            answer.body
        );
        // The refused request wrote nothing: the setting still reports what it
        // reports for an account that was never given it — nothing at all, or the
        // default spend limit for the one setting an account always has.
        let reported = read(&sns, case, &[]).await;
        let expected = (*name == MONTHLY_SPEND_LIMIT_ATTRIBUTE).then_some(DEFAULT_MONTHLY_SPEND_LIMIT);
        assert_eq!(
            reported.get(*name).map(String::as_str),
            expected,
            "case {case:?}: a refused write must store nothing, got {reported:?}"
        );
    }
}

/// One refused multi-entry write: the case, the entries of one call, and the whole
/// message the refusal carries.
struct BadRequest<'a> {
    case: &'static str,
    entries: Vec<(&'a str, &'a str)>,
    message: String,
}

/// A request carrying one unacceptable entry changes NOTHING, however acceptable its
/// siblings are: names and values are judged before anything is written.
#[tokio::test]
async fn one_bad_entry_refuses_the_whole_request() {
    let cases = [
        BadRequest {
            case: "an acceptable setting beside a name that is no setting",
            entries: vec![
                (DEFAULT_SMS_TYPE_ATTRIBUTE, "Transactional"),
                ("NotARealSetting", "whatever"),
            ],
            message: "NotARealSetting is not a valid attribute".to_string(),
        },
        BadRequest {
            case: "an acceptable setting beside an unacceptable value",
            entries: vec![
                (DEFAULT_SENDER_ID_ATTRIBUTE, "Brand7"),
                (DEFAULT_SMS_TYPE_ATTRIBUTE, "Marketing"),
            ],
            message: format!("{DEFAULT_SMS_TYPE_ATTRIBUTE} is invalid"),
        },
    ];

    for BadRequest {
        case,
        entries,
        message,
    } in &cases
    {
        let sns = Sns::new();
        let answer = set(&sns, entries).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(message.as_str()),
            "case {case:?}: expected the refusal {message:?}, got {}",
            answer.body
        );
        let reported = read(&sns, case, &[]).await;
        for (name, _) in entries {
            let expected =
                (*name == MONTHLY_SPEND_LIMIT_ATTRIBUTE).then_some(DEFAULT_MONTHLY_SPEND_LIMIT);
            assert_eq!(
                reported.get(*name).map(String::as_str),
                expected,
                "case {case:?}: no entry of a refused request may be written, got {reported:?}"
            );
        }
    }
}
