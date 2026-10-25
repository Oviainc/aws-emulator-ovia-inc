// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RegisterInstanceEventNotificationAttributes`,
//! `DeregisterInstanceEventNotificationAttributes` and
//! `DescribeInstanceEventNotificationAttributes`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the tag keys registered to appear in scheduled event
//! notifications are ONE registration per Region — either every tag key of that Region is
//! registered (`IncludeAllTagsOfInstance`) or exactly the keys registered individually are
//! (`InstanceTagKeys`) — that a register UNIONS the keys it names into it, that a
//! deregister REMOVES the keys it names from it (and, with `IncludeAllTagsOfInstance=false`,
//! every one of them) while answering the keys it took out, and that the describe of that
//! Region reports what the registration holds afterwards.
//!
//! So the tables below span that class rather than one recorded request: key sets that
//! differ in count, in content, in the order they arrive in and in whether they repeat a
//! key; keys nothing registered; both modes of the attribute, in both orders; several
//! Regions; and the published tag-key length bound with one character past it.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, indexed};
use super::wire::{child, child_text, document, param, Param};

/// The Region most calls here are signed for: the one the default network is seeded for.
const REGION: &str = "us-east-1";

/// `RegisterInstanceEventNotificationAttributes`.
const REGISTER: &str = "RegisterInstanceEventNotificationAttributes";

/// `DeregisterInstanceEventNotificationAttributes`.
const DEREGISTER: &str = "DeregisterInstanceEventNotificationAttributes";

/// `DescribeInstanceEventNotificationAttributes`.
const DESCRIBE: &str = "DescribeInstanceEventNotificationAttributes";

/// The longest tag key EC2 accepts, from this crate's own bound
/// (`ec2::state::MAX_TAG_KEY_CHARS`, "A tag key accepts at most 127 characters").
const MAX_KEY_CHARS: usize = ec2::state::MAX_TAG_KEY_CHARS;

/// One Region's registration as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Registration {
    /// `includeAllTagsOfInstance`.
    include_all: bool,
    /// `instanceTagKeySet`, in the order the response listed them.
    keys: Vec<String>,
}

impl Registration {
    /// The registration of a Region nothing has registered in: no key, and not every key.
    fn empty() -> Self {
        Self {
            include_all: false,
            keys: Vec::new(),
        }
    }

    /// The registration that holds exactly `keys`, whatever order they are given in: the
    /// set a sequence of registers builds up.
    fn of(keys: &[&str]) -> Self {
        let mut keys: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
        keys.sort();
        keys.dedup();
        Self {
            include_all: false,
            keys,
        }
    }

    /// The registration that holds every tag key of the Region: "true indicates that all
    /// tag keys in the current Region are registered", which leaves no key to name.
    fn all() -> Self {
        Self {
            include_all: true,
            keys: Vec::new(),
        }
    }

    /// These keys in one canonical order, so a comparison is about the SET the service
    /// holds and not about an order AWS does not document.
    fn sorted(&self) -> Self {
        let mut sorted = self.clone();
        sorted.keys.sort();
        sorted
    }
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The form fields of a register that names `keys` individually: ec2Query flattens
/// `InstanceTagKeys` under its `InstanceTagKey` XML name, inside the `InstanceTagAttribute`
/// structure — indexed from 1, which is what [`super::ec2_query::indexed`] builds.
///
/// The deregister's own `InstanceTagKeys` carries the same XML name inside the same
/// `InstanceTagAttribute` structure, so this builds the key fields of either request.
fn register_keys(keys: &[&str]) -> Vec<Param> {
    indexed("InstanceTagAttribute.InstanceTagKey", keys)
}

/// The form fields of a register that asks for every tag key of the Region.
fn register_all() -> Vec<Param> {
    vec![param("InstanceTagAttribute.IncludeAllTagsOfInstance", "true")]
}

/// The form fields of a deregister that asks for every tag key of the Region: "Specify
/// `false` to deregister all tag keys"
/// (`DeregisterInstanceTagAttributeRequest$IncludeAllTagsOfInstance`), which is the mirror of
/// the register's `true`.
fn deregister_all() -> Vec<Param> {
    vec![param("InstanceTagAttribute.IncludeAllTagsOfInstance", "false")]
}

/// The registration a response carries, or a panic naming the case: the one
/// `<instanceTagAttribute>` element both operations answer with.
fn registration(case: &str, body: &str) -> Registration {
    let parsed = document(case, body);
    let attribute = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "instanceTagAttribute")
        .unwrap_or_else(|| {
            panic!("case {case:?}: no <instanceTagAttribute> in {body}")
        });
    let include_all = child_text(attribute, "includeAllTagsOfInstance").unwrap_or_else(|| {
        panic!("case {case:?}: no <includeAllTagsOfInstance> in {body}")
    });
    let keys = child(attribute, "instanceTagKeySet")
        .map(|set| {
            set.children()
                .filter(|node| node.is_element() && node.tag_name().name() == "item")
                .map(|item| item.text().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();

    Registration {
        include_all: match include_all.as_str() {
            "true" => true,
            "false" => false,
            other => panic!("case {case:?}: <includeAllTagsOfInstance> carried {other:?}"),
        },
        keys,
    }
}

/// One register call in `region`, required to succeed, as the registration it answers with.
async fn register(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    params: &[Param],
) -> Registration {
    let (status, body) = call(router, region, REGISTER, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {REGISTER}, got {status} with body {body}"
    );
    registration(case, &body)
}

/// The registration `region` reports, required to succeed.
async fn describe(router: &MultiServiceRouter, case: &str, region: &str) -> Registration {
    let (status, body) = call(router, region, DESCRIBE, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {DESCRIBE}, got {status} with body {body}"
    );
    registration(case, &body)
}

/// One deregister call in `region`, required to succeed, as the attribute it answers with.
async fn deregister(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    params: &[Param],
) -> Registration {
    let (status, body) = call(router, region, DEREGISTER, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {DEREGISTER}, got {status} with body {body}"
    );
    registration(case, &body)
}

/// How many Regions the store holds a registration ENTRY for, read off the state the
/// emulator serves from: an emptied registration must leave none behind.
fn stored_regions(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
        .lock()
        .instance_event_tag_registrations
        .len()
}

/// The registration of `region`, asserted against `expected` as a SET.
async fn assert_registration(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    expected: &Registration,
) {
    let actual = describe(router, case, region).await;
    assert_eq!(
        actual.sorted(),
        expected.sorted(),
        "case {case:?}: expected {region} to report {expected:?}, got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// The rule: a register of individual tag keys stores exactly those keys for the Region,
/// answers with the resulting registration, and the describe of that Region reports the
/// same thing back.
///
/// The rows differ along what the rule generalises over — how many keys, what they contain
/// (an AWS-generated key, a key with punctuation, a non-ASCII key), the order they arrive
/// in, and whether one key is named twice in one request.
#[tokio::test]
async fn a_register_stores_the_keys_it_names_and_the_describe_reports_them_back() {
    // (case, the keys the register names)
    let cases: [(&str, &[&str]); 7] = [
        ("one key", &["Name"]),
        ("two keys", &["Name", "Stage"]),
        ("five keys", &["a", "b", "c", "d", "e"]),
        (
            "keys in descending order",
            &["zeta", "mu", "epsilon", "alpha"],
        ),
        ("one key named twice", &["Owner", "Owner"]),
        (
            "keys with punctuation and an AWS-generated key",
            &["aws:cloudformation:stack-name", "cost.center/team-1"],
        ),
        ("a non-ASCII key", &["Eigentümer", "所有者"]),
    ];

    for (case, keys) in cases {
        // A fresh emulator per row, so a row starts from a Region with nothing registered.
        let router = emulator();
        assert_registration(&router, case, REGION, &Registration::empty()).await;

        let expected = Registration::of(keys);
        let answered = register(&router, case, REGION, &register_keys(keys)).await;
        assert_eq!(
            answered.sorted(),
            expected.sorted(),
            "case {case:?}: expected {REGISTER} to answer {expected:?}, got {answered:?}"
        );
        assert_registration(&router, case, REGION, &expected).await;
    }
}

/// The keys ACCUMULATE: each register unions the keys it names into the ones already
/// registered, so registering one key never deregisters another, and a key registered twice
/// is held once.
///
/// The rows are the sequence's steps, applied in order to one Region; the expectation after
/// each step is computed from the steps so far rather than written out.
#[tokio::test]
async fn each_register_unions_its_keys_into_the_registration() {
    let router = emulator();
    // (case, the keys this step registers)
    let steps: [(&str, &[&str]); 5] = [
        ("the first key", &["Name"]),
        ("a second key", &["Stage"]),
        ("the first key again", &["Name"]),
        ("two more keys at once", &["Owner", "Tier"]),
        ("a key that is already registered, with a new one", &["Tier", "Fleet"]),
    ];

    let mut registered: Vec<&str> = Vec::new();
    for (case, keys) in steps {
        registered.extend_from_slice(keys);
        let expected = Registration::of(&registered);
        let answered = register(&router, case, REGION, &register_keys(keys)).await;
        assert_eq!(
            answered.sorted(),
            expected.sorted(),
            "case {case:?}: expected {REGISTER} to answer {expected:?}, got {answered:?}"
        );
        assert_registration(&router, case, REGION, &expected).await;
    }
}

/// The two modes of the attribute are ALTERNATIVES, and the last register decides which one
/// the Region is in: registering all tag keys reports every key registered and names none,
/// and registering individual keys afterwards puts the Region back to exactly those keys.
///
/// The rows are the sequence's steps, each stating the whole registration it leaves behind.
#[tokio::test]
async fn registering_all_tag_keys_and_registering_individual_keys_replace_each_other() {
    let router = emulator();
    // (case, the register's form fields, the registration it must leave behind)
    let steps: [(&str, Vec<Param>, Registration); 5] = [
        ("all tag keys from empty", register_all(), Registration::all()),
        ("all tag keys again", register_all(), Registration::all()),
        (
            "one individual key after all of them",
            register_keys(&["Name"]),
            Registration::of(&["Name"]),
        ),
        (
            "a second individual key",
            register_keys(&["Stage"]),
            Registration::of(&["Name", "Stage"]),
        ),
        (
            "all tag keys again, which subsumes the individual ones",
            register_all(),
            Registration::all(),
        ),
    ];

    for (case, params, expected) in steps {
        let answered = register(&router, case, REGION, &params).await;
        assert_eq!(
            answered.sorted(),
            expected.sorted(),
            "case {case:?}: expected {REGISTER} to answer {expected:?}, got {answered:?}"
        );
        assert_registration(&router, case, REGION, &expected).await;
    }
}

/// The registration is a fact of a REGION: the describe reports the keys registered "for
/// resources in the current Region", so a register in one Region leaves every other Region
/// as it was.
///
/// The rows are Regions that differ in their partition prefix and in whether anything was
/// registered there at all.
#[tokio::test]
async fn the_registration_belongs_to_the_region_the_request_was_signed_for() {
    let router = emulator();
    // (case, the Region, what is registered there, what it must then report)
    let cases: [(&str, &str, Vec<Param>, Registration); 4] = [
        (
            "individual keys in us-east-1",
            "us-east-1",
            register_keys(&["Name", "Stage"]),
            Registration::of(&["Name", "Stage"]),
        ),
        (
            "a different key in eu-west-2",
            "eu-west-2",
            register_keys(&["Owner"]),
            Registration::of(&["Owner"]),
        ),
        (
            "all tag keys in ap-southeast-1",
            "ap-southeast-1",
            register_all(),
            Registration::all(),
        ),
        (
            "nothing at all in us-west-2",
            "us-west-2",
            Vec::new(),
            Registration::empty(),
        ),
    ];

    for (case, region, params, _) in &cases {
        if params.is_empty() {
            continue;
        }
        register(&router, case, region, params).await;
    }
    // Every Region still reports its own registration, after every other Region was written.
    for (case, region, _, expected) in &cases {
        assert_registration(&router, case, region, expected).await;
    }
}

/// A describe is a READ: describing a Region repeatedly reports the same registration and
/// registers nothing — including for a Region nothing has ever registered in.
#[tokio::test]
async fn describing_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    // (case, the Region, what it must report every time)
    let cases: [(&str, &str, Registration); 2] = [
        ("a Region nothing has registered in", "sa-east-1", Registration::empty()),
        (
            "a Region with keys registered",
            REGION,
            Registration::of(&["Name"]),
        ),
    ];
    register(&router, "seeding the second case", REGION, &register_keys(&["Name"])).await;

    for (case, region, expected) in &cases {
        let first = describe(&router, case, region).await;
        let second = describe(&router, case, region).await;
        assert_eq!(
            first, second,
            "case {case:?}: two describes of {region} must report the same registration, \
             got {first:?} then {second:?}"
        );
        assert_eq!(
            first.sorted(),
            expected.sorted(),
            "case {case:?}: expected {region} to report {expected:?}, got {first:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The deregister
// ---------------------------------------------------------------------------

/// The attribute a deregister answers with: the tag keys it took OUT of the registration, as
/// a set, beside the mode the registration is LEFT in — derived from the row rather than
/// written out twice.
fn removed(keys: &[&str], left: &Registration) -> Registration {
    let mut answer = Registration::of(keys);
    answer.include_all = left.include_all;
    answer
}

/// The rule: a deregister removes exactly the tag keys it names from the Region's
/// registration, leaves every other key where it is, and answers the keys it removed — which
/// the registration no longer holds, so only the describe reports what is left.
///
/// The rows differ along what the rule generalises over: how many keys of how many the
/// request names, a key nothing registered, the same key named twice, keys with punctuation
/// and non-ASCII keys, and a registration in the include-all mode, whose mode naming keys
/// must not turn off.
#[tokio::test]
async fn a_deregister_removes_the_keys_it_names_and_answers_the_keys_it_removed() {
    // (case, what is registered first, the keys the deregister names, what is left)
    let cases: [(&str, Vec<Param>, &[&str], Registration); 7] = [
        (
            "one of three keys",
            register_keys(&["Name", "Stage", "Owner"]),
            &["Stage"],
            Registration::of(&["Name", "Owner"]),
        ),
        (
            "two of three keys in one request",
            register_keys(&["Name", "Stage", "Owner"]),
            &["Name", "Owner"],
            Registration::of(&["Stage"]),
        ),
        (
            "the only registered key",
            register_keys(&["Name"]),
            &["Name"],
            Registration::empty(),
        ),
        (
            "every registered key, named explicitly",
            register_keys(&["Name", "Stage"]),
            &["Stage", "Name"],
            Registration::empty(),
        ),
        (
            "a key nothing registered",
            register_keys(&["Name"]),
            &["Absent"],
            Registration::of(&["Name"]),
        ),
        (
            "the same key named twice in one request",
            register_keys(&["cost.center/team-1", "aws:cloudformation:stack-name"]),
            &["cost.center/team-1", "cost.center/team-1"],
            Registration::of(&["aws:cloudformation:stack-name"]),
        ),
        (
            // The include-all mode holds no individual key, so naming keys removes none —
            // and the mode is not what naming keys asks about, so it stays on.
            "a non-ASCII key against the include-all mode",
            register_all(),
            &["Eigentümer"],
            Registration::all(),
        ),
    ];

    for (case, seed, keys, left) in cases {
        // A fresh emulator per row, so a row starts from a Region with nothing registered.
        let (router, engine) = super::ec2_query::emulator();
        register(&router, case, REGION, &seed).await;

        let expected = removed(keys, &left);
        let answered = deregister(&router, case, REGION, &register_keys(keys)).await;
        assert_eq!(
            answered.sorted(),
            expected.sorted(),
            "case {case:?}: expected {DEREGISTER} to answer {expected:?}, got {answered:?}"
        );
        assert_registration(&router, case, REGION, &left).await;

        // A registration with nothing left in it is one nothing has registered in, so the
        // store keeps no entry for the Region at all.
        let stored_expected = usize::from(left != Registration::empty());
        let stored = stored_regions(&engine);
        assert_eq!(
            stored, stored_expected,
            "case {case:?}: expected the store to hold {stored_expected} entr(y/ies) for \
             {left:?}, got {stored}"
        );
    }
}

/// `IncludeAllTagsOfInstance=false` deregisters ALL tag keys ("Specify `false` to deregister
/// all tag keys"), whichever mode the Region's registration is in, and answers the keys it
/// took out.
///
/// The rows are the states a registration can be in: individual keys, the include-all mode,
/// and a Region nothing has registered in at all.
#[tokio::test]
async fn deregistering_all_tag_keys_empties_the_registration_whichever_mode_it_is_in() {
    // (case, what is registered first, the keys the deregister must answer with)
    let cases: [(&str, Vec<Param>, &[&str]); 4] = [
        (
            "two individually registered keys",
            register_keys(&["Name", "Stage"]),
            &["Name", "Stage"],
        ),
        ("one individually registered key", register_keys(&["Name"]), &["Name"]),
        // The include-all mode holds no individual key, so there is none to report.
        ("the include-all mode", register_all(), &[]),
        ("a Region nothing has registered in", Vec::new(), &[]),
    ];

    for (case, seed, answered_keys) in cases {
        let (router, engine) = super::ec2_query::emulator();
        if !seed.is_empty() {
            register(&router, case, REGION, &seed).await;
        }

        let expected = Registration::of(answered_keys);
        let answered = deregister(&router, case, REGION, &deregister_all()).await;
        assert_eq!(
            answered.sorted(),
            expected.sorted(),
            "case {case:?}: expected {DEREGISTER} to answer {expected:?}, got {answered:?}"
        );
        assert_registration(&router, case, REGION, &Registration::empty()).await;
        assert_eq!(
            stored_regions(&engine),
            0,
            "case {case:?}: expected a Region with nothing registered to hold no entry"
        );
    }
}

/// A deregister is a fact of the Region the request was signed for: it reaches only that
/// Region's registration, and every other Region is left exactly as it was.
///
/// The rows are Regions in different partitions, one in each mode of the attribute.
#[tokio::test]
async fn a_deregister_reaches_only_the_region_it_was_signed_for() {
    let router = emulator();
    // (case, the Region, what is registered there, what it must report after the deregister
    // in the first Region)
    let cases: [(&str, &str, Vec<Param>, Registration); 3] = [
        (
            "the Region the deregister is signed for",
            "us-east-1",
            register_keys(&["Name", "Stage"]),
            Registration::of(&["Stage"]),
        ),
        (
            "another Region with the same key registered",
            "eu-west-2",
            register_keys(&["Name", "Stage"]),
            Registration::of(&["Name", "Stage"]),
        ),
        (
            "a Region in the include-all mode",
            "ap-southeast-1",
            register_all(),
            Registration::all(),
        ),
    ];

    for (case, region, seed, _) in &cases {
        register(&router, case, region, seed).await;
    }
    deregister(
        &router,
        "the deregister under test",
        "us-east-1",
        &register_keys(&["Name"]),
    )
    .await;
    for (case, region, _, expected) in &cases {
        assert_registration(&router, case, region, expected).await;
    }
}

/// Every refusal the deregister makes, each with the EC2 error code and HTTP status it
/// answers — the same ones the register answers for the same bad request, since the two
/// requests carry the same attribute — and none of them writes anything.
///
/// The rows differ along what makes the request bad: both members of the attribute in one
/// request, neither of them, no attribute at all, a key with no characters, a key one
/// character past the published bound, and a dry run of each of the deregister's two modes.
#[tokio::test]
async fn a_bad_deregister_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    let seeded = Registration::of(&["Name"]);
    register(&router, "the refusal table's key", REGION, &register_keys(&["Name"])).await;

    let mut both = deregister_all();
    both.extend(register_keys(&["Name"]));
    let mut both_on = vec![param("InstanceTagAttribute.IncludeAllTagsOfInstance", "true")];
    both_on.extend(register_keys(&["Name"]));
    let past_bound: String = std::iter::repeat_n('k', MAX_KEY_CHARS + 1).collect();
    // (case, the deregister's form fields, the HTTP status, the EC2 error code)
    let cases: [(&str, Vec<Param>, u16, &str); 8] = [
        (
            "both members of the attribute",
            both,
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            // The flag carries the other value, which this request has no reading for —
            // but it is still the flag, named beside a tag key.
            "both members of the attribute, with the flag the other way round",
            both_on,
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            // `true` is not a value this request has a reading for: "Specify `false` to
            // deregister all tag keys", so the request names neither member.
            "neither member of the attribute",
            vec![param("InstanceTagAttribute.IncludeAllTagsOfInstance", "true")],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            // `InstanceTagAttribute` is a modelled `@required` member, so the boundary
            // validator refuses a request without it BEFORE the handler runs — with its
            // own `MalformedInput`, not with the handler's `MissingParameter`.
            "no attribute at all",
            Vec::new(),
            400,
            "MalformedInput",
        ),
        (
            "a key with no characters",
            register_keys(&[""]),
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a key one character past the published length bound",
            register_keys(&[past_bound.as_str()]),
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a dry run of a deregister of one key that would have succeeded",
            {
                let mut params = register_keys(&["Name"]);
                params.push(param("DryRun", "true"));
                params
            },
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a deregister of all tag keys",
            {
                let mut params = deregister_all();
                params.push(param("DryRun", "true"));
                params
            },
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (case, params, status_expected, code_expected) in cases {
        let (status, body) = call(&router, REGION, DEREGISTER, &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {DEREGISTER}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?} from {DEREGISTER}, got \
             {code:?} with body {body}"
        );
        // Nothing the refusal named was written: the Region still reports what it held.
        assert_registration(&router, case, REGION, &seeded).await;
    }
}

// ---------------------------------------------------------------------------
// The bound on a tag key, and the refusals
// ---------------------------------------------------------------------------

/// A registered key is a tag key, so it is accepted up to the published bound
/// (`ec2::state::MAX_TAG_KEY_CHARS` — "A tag key accepts at most 127 characters") and
/// refused one character past it.
///
/// The rows are that bound, one below it and one above it, each measured in CHARACTERS so a
/// multi-byte key at the bound is accepted too.
#[tokio::test]
async fn a_registered_tag_key_is_accepted_up_to_the_published_length_bound() {
    // (case, the key's length in characters, the character it is built from, the HTTP
    // status, the EC2 error code)
    let cases: [(&str, usize, char, u16, &str); 4] = [
        ("one below the bound", MAX_KEY_CHARS - 1, 'k', 200, ""),
        ("exactly at the bound", MAX_KEY_CHARS, 'k', 200, ""),
        (
            "at the bound in multi-byte characters",
            MAX_KEY_CHARS,
            'ü',
            200,
            "",
        ),
        (
            "one past the bound",
            MAX_KEY_CHARS + 1,
            'k',
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, length, character, status_expected, code_expected) in cases {
        let router = emulator();
        let key: String = std::iter::repeat_n(character, length).collect();
        let (status, body) = call(&router, REGION, REGISTER, &register_keys(&[&key])).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {REGISTER} for a \
             {length}-character key, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?}, got {code:?} with \
             body {body}"
        );
        let expected = if status_expected == 200 {
            Registration::of(&[&key])
        } else {
            Registration::empty()
        };
        assert_registration(&router, case, REGION, &expected).await;
    }
}

/// The store of registrations is bounded in ENTRIES, not only in bytes: the Region an entry
/// is keyed by comes from the request's own credential scope, and a registration in the
/// include-all mode holds no tag key at all, so a register signed for a Region past the
/// store's ceiling (`ec2::state::MAX_EVENT_NOTIFICATION_REGIONS`) is refused — while a
/// Region that already has an entry can still be re-registered, since that adds none.
#[tokio::test]
async fn the_number_of_regions_the_store_registers_keys_for_is_bounded() {
    let (router, engine) = super::ec2_query::emulator();
    // Filled through the state's own write seam rather than over the wire, because what is
    // under test is the BOUNDARY the handler enforces, not the fill: the register at, below
    // and past the ceiling below are the real ec2Query calls. Filled in the include-all
    // mode, whose registration holds NO tag key, so the byte budget cannot be what refuses
    // the entry past the ceiling.
    {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        for nth in 0..ec2::state::MAX_EVENT_NOTIFICATION_REGIONS {
            let region = format!("em-ulator-{nth}");
            assert!(
                guard.has_instance_event_region_capacity(&region),
                "filling the store: expected room for {region}, held {} entries",
                nth
            );
            guard.set_instance_event_tag_registration(
                &region,
                ec2::state::InstanceEventTagRegistration::registering_all(),
            );
        }
    }

    // (case, the Region the register is signed for, the HTTP status, the EC2 error code)
    let cases: [(&str, String, u16, &str); 3] = [
        (
            "a Region the store already holds an entry for",
            "em-ulator-0".to_string(),
            200,
            "",
        ),
        (
            "the last Region the store took",
            format!("em-ulator-{}", ec2::state::MAX_EVENT_NOTIFICATION_REGIONS - 1),
            200,
            "",
        ),
        (
            // `TagLimitExceeded` is the EC2 quota code for tag keys the store cannot take.
            "a Region past the ceiling",
            "em-ulator-over-the-ceiling".to_string(),
            400,
            "TagLimitExceeded",
        ),
    ];

    for (case, region, status_expected, code_expected) in cases {
        let (status, body) = call(&router, &region, REGISTER, &register_keys(&["Name"])).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {REGISTER} in {region}, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?} from {REGISTER} in \
             {region}, got {code:?} with body {body}"
        );
        let expected = if status_expected == 200 {
            Registration::of(&["Name"])
        } else {
            Registration::empty()
        };
        assert_registration(&router, case, &region, &expected).await;
    }
}

/// Every refusal these two operations make, each with the EC2 error code and HTTP status it
/// answers — and none of them writes anything.
///
/// The rows differ along what makes the request bad: both members of the attribute in one
/// request ("You must specify either IncludeAllTagsOfInstance or InstanceTagKeys"), neither
/// of them, an attribute that is not in the request at all, a key with no characters, and a
/// dry run of each operation.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    let seeded = Registration::of(&["Name"]);
    register(&router, "the refusal table's key", REGION, &register_keys(&["Name"])).await;

    let mut both = register_all();
    both.extend(register_keys(&["Stage"]));
    // (case, the action, its form fields, the HTTP status, the EC2 error code)
    let cases: [(&str, &str, Vec<Param>, u16, &str); 6] = [
        (
            "both members of the attribute",
            REGISTER,
            both,
            400,
            ec2::state::INVALID_PARAMETER_COMBINATION,
        ),
        (
            "neither member of the attribute",
            REGISTER,
            vec![param("InstanceTagAttribute.IncludeAllTagsOfInstance", "false")],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            // `InstanceTagAttribute` is a modelled `@required` member, so the boundary
            // validator refuses a request without it BEFORE the handler runs — with its
            // own `MalformedInput`, not with the handler's `MissingParameter`.
            "no attribute at all",
            REGISTER,
            Vec::new(),
            400,
            "MalformedInput",
        ),
        (
            "a key with no characters",
            REGISTER,
            register_keys(&[""]),
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a dry run of a register that would have succeeded",
            REGISTER,
            {
                let mut params = register_keys(&["Stage"]);
                params.push(param("DryRun", "true"));
                params
            },
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a describe that would have succeeded",
            DESCRIBE,
            vec![param("DryRun", "true")],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (case, action, params, status_expected, code_expected) in cases {
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {action}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?} from {action}, got \
             {code:?} with body {body}"
        );
        // Nothing the refusal named was written: the Region still reports what it held.
        assert_registration(&router, case, REGION, &seeded).await;
    }
}
