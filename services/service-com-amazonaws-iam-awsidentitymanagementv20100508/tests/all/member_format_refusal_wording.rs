// SPDX-License-Identifier: CC0-1.0

//! The refusals IAM states as ONE fixed sentence for a MEMBER's whole form, end to end
//! over the awsQuery wire.
//!
//! The rule under test: a value whose form is wrong for the member it was supplied for —
//! where AWS publishes one sentence for that member's whole format — is refused HTTP 400
//! `ValidationError` whose message is exactly that sentence (`The specified value for
//! <member> is invalid. It must <the published form>`), carrying nothing of the submitted
//! value and stating neither the model's `N validation error(s) detected:` envelope nor
//! any per-constraint clause. Two consequences the rows read back:
//!
//! 1. the sentence names the MEMBER, so it is the same sentence for every operation
//!    carrying that member whatever shape the member targets there, and for every way the
//!    value is outside the member's form — its length and its alphabet alike, because one
//!    sentence states the whole of it;
//! 2. these members are taken by IAM itself rather than by the model-level constraint
//!    walk, so their constraints are never stated as a clause: a request that ALSO broke a
//!    member that walk DOES take is answered by the walk, over that member's clauses alone
//!    (`tests/all/range_refusal_wording.rs` is the table for those clauses).
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! member carries the form and which operation takes it, which shape the member targets
//! (so which published form is broken), and HOW the value is outside it — the wrong first
//! character, a character outside the alphabet, an empty value, a value under the
//! published minimum length and one past the published maximum. No single request is the
//! specification; the well-formed rows show the sentence is not a blanket refusal of the
//! member.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the
//! one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// AWS's sentence for a `pathPrefix` whose form is wrong, written out here rather than
/// read from the implementation's constant: what this suite pins is the SENTENCE AWS
/// answers, so a reworded constant has to fail here rather than agree with itself.
const INVALID_PATH_PREFIX: &str = "The specified value for pathPrefix is invalid. It must \
                                   begin with the / character and contain only alphanumeric \
                                   characters and/or / characters.";

/// AWS's sentence for an `accountAlias` whose form is wrong — one sentence for the whole
/// of that member's form, its length and its alphabet together.
const INVALID_ACCOUNT_ALIAS: &str =
    "The specified value for accountAlias is invalid. It must be a minimum length of 3 \
     characters and maximum length of 63 characters, contain only digits, lowercase letters, \
     and hyphens (-), but cannot begin or end with a hyphen.";

/// The wire code the query protocol names a refused value with, which this service answers
/// these refusals under: a client branches on the code, so the sentence is not the only
/// thing a row pins.
const VALUE_REFUSAL_CODE: &str = "ValidationError";

/// One row: the call to make, and the malformed value it carries, which the refusal must
/// not echo.
struct FormCase {
    case: String,
    action: &'static str,
    params: Vec<(String, String)>,
    rejected: String,
    expected: &'static str,
}

/// Assert `row` is refused with the fixed sentence for its member and with nothing of the
/// value it carried.
async fn assert_refused(router: &MultiServiceRouter, row: FormCase) {
    let FormCase {
        case,
        action,
        params,
        rejected,
        expected,
    } = row;
    let (status, body) = call(router, action, &borrowed(&params)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        VALUE_REFUSAL_CODE,
        "case {case:?}: expected the query protocol's refusal code, got {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    // Nothing of the submitted value, however it was spelled, and no clause of the
    // model-level envelope either.
    assert!(
        !body.contains(&format!("Value &apos;{rejected}&apos;"))
            && !body.contains(&format!("Value '{rejected}'")),
        "case {case:?}: the refusal must not echo {rejected:?}, got {body}"
    );
    assert!(
        !message.contains("validation error"),
        "case {case:?}: expected no model-level envelope, got {message:?}"
    );
}

/// The rows: a `pathPrefix` outside its form on each of the three shapes this service's
/// listings target with that member, over the operations that carry it.
fn path_prefix_cases() -> Vec<FormCase> {
    // A policy the account does not hold: the refusal is stated before any handler runs,
    // so what the ARN names never matters, and its own form is admissible.
    let policy_arn = format!("arn:aws:iam::{}:policy/a-policy-not-held", account_id());
    let row = |case: &str, action: &'static str, params: Vec<(String, String)>, rejected: &str| {
        FormCase {
            case: case.to_string(),
            action,
            params,
            rejected: rejected.to_string(),
            expected: INVALID_PATH_PREFIX,
        }
    };
    vec![
        // `pathPrefixType` (length 1..512, pattern ^\u002F[\u0021-\u007F]*$): a prefix that
        // does not begin with the / character, on each listing that takes it.
        row(
            "a group listing's prefix with no leading slash",
            "ListGroups",
            vec![param("PathPrefix", "badpath")],
            "badpath",
        ),
        row(
            "a role listing's prefix with no leading slash",
            "ListRoles",
            vec![param("PathPrefix", "eng")],
            "eng",
        ),
        row(
            "an instance-profile listing's prefix with no leading slash",
            "ListInstanceProfiles",
            vec![param("PathPrefix", "badpath")],
            "badpath",
        ),
        // The same member, outside the same form a different way: a character outside the
        // alphabet, and an EMPTY value, which is under the published minimum length.
        row(
            "a user listing's prefix carrying a space",
            "ListUsers",
            vec![param("PathPrefix", "/has space/")],
            "/has space/",
        ),
        row(
            "a server-certificate listing's prefix carrying a non-ASCII character",
            "ListServerCertificates",
            vec![param("PathPrefix", "/caf\u{e9}/")],
            "/caf\u{e9}/",
        ),
        row(
            "an empty group listing prefix",
            "ListGroups",
            vec![param("PathPrefix", "")],
            "",
        ),
        // The same member beside other members of the same request, so the sentence is
        // the whole answer rather than one clause of it.
        row(
            "a role listing's prefix beside a page size",
            "ListRoles",
            vec![param("PathPrefix", "eng/"), param("MaxItems", "10")],
            "eng/",
        ),
        // `policyPathType` (pattern ^((/[A-Za-z0-9\.,\+@=_-]+)*)/$): the same member on a
        // policy listing, where the published form it targets is a different one.
        row(
            "a policy listing's prefix that does not end with a slash",
            "ListPolicies",
            vec![param("PathPrefix", "/eng")],
            "/eng",
        ),
        row(
            "an attached-policy listing's prefix with no leading slash",
            "ListAttachedRolePolicies",
            vec![param("RoleName", "a-role"), param("PathPrefix", "eng/")],
            "eng/",
        ),
        row(
            "an attached-policy listing's prefix carrying a space",
            "ListAttachedUserPolicies",
            vec![param("UserName", "a-user"), param("PathPrefix", "/has it/")],
            "/has it/",
        ),
        row(
            "a group's attached-policy listing prefix with an empty segment",
            "ListAttachedGroupPolicies",
            vec![param("GroupName", "a-group"), param("PathPrefix", "/eng//")],
            "/eng//",
        ),
        // `pathType` (pattern ^(\u002F)|(\u002F[\u0021-\u007E]+\u002F)$): the same member
        // on the entity listing, which filters by the related entity's own path.
        row(
            "an entity listing's prefix with no slash at all",
            "ListEntitiesForPolicy",
            vec![param("PolicyArn", &policy_arn), param("PathPrefix", "eng")],
            "eng",
        ),
    ]
}

/// RULE: a `pathPrefix` outside the form published for it is refused 400 `ValidationError`
/// with AWS's one sentence for that member, on every operation that takes the member and
/// whatever shape it targets there.
#[tokio::test]
async fn a_path_prefix_outside_its_published_form_is_refused_with_the_fixed_sentence() {
    let router = router();
    for row in path_prefix_cases() {
        assert_refused(&router, row).await;
    }
}

/// RULE: the refusal is about the FORM, so a prefix inside it is served — the fixed
/// sentence is not a blanket refusal of the member.
#[tokio::test]
async fn a_well_formed_path_prefix_is_served() {
    let router = router();
    // Rows: the account root, a one-segment prefix and a nested one, on each listing whose
    // `pathPrefix` the suite above refuses and which answers without a fixture.
    for action in [
        "ListGroups",
        "ListRoles",
        "ListUsers",
        "ListInstanceProfiles",
        "ListServerCertificates",
        "ListPolicies",
    ] {
        for prefix in ["/", "/eng/", "/eng/tools/"] {
            let (status, body) = call(&router, action, &[("PathPrefix", prefix)]).await;
            assert_eq!(
                status, 200,
                "case {action:?} with {prefix:?}: expected HTTP 200, got {status}: {body}"
            );
        }
    }
}

/// The malformed aliases: one row per way a value can be outside `accountAliasType`'s
/// published form — under the minimum length, past the maximum, outside the alphabet, and
/// each placement of a dash the form forbids.
fn malformed_aliases() -> Vec<(String, String)> {
    vec![
        ("one character under the published minimum".to_string(), "ab".to_string()),
        (
            "one character past the published maximum".to_string(),
            format!("a{}c", "b".repeat(62)),
        ),
        ("an upper-case letter".to_string(), "MyAlias".to_string()),
        ("a leading dash".to_string(), "-my-alias".to_string()),
        ("a trailing dash".to_string(), "my-alias-".to_string()),
        ("two dashes in a row".to_string(), "my--alias".to_string()),
        ("an underscore".to_string(), "my_alias".to_string()),
        ("a space".to_string(), "my alias".to_string()),
        ("a dot".to_string(), "my.alias".to_string()),
        ("a non-ASCII letter".to_string(), "mon-ali\u{e9}s".to_string()),
    ]
}

/// RULE: an `accountAlias` outside the form published for it is refused 400
/// `ValidationError` with AWS's one sentence for that member — one sentence for the whole
/// form, so the length rows and the alphabet rows answer alike — on the create and on the
/// delete, and the account holds nothing differently afterwards.
#[tokio::test]
async fn an_account_alias_outside_its_published_form_is_refused_with_the_fixed_sentence() {
    let router = router();
    for action in ["CreateAccountAlias", "DeleteAccountAlias"] {
        for (defect, alias) in malformed_aliases() {
            let row = FormCase {
                case: format!("{action} with {defect}"),
                action,
                params: vec![param("AccountAlias", &alias)],
                rejected: alias.clone(),
                expected: INVALID_ACCOUNT_ALIAS,
            };
            assert_refused(&router, row).await;
        }
    }

    // A refused request persisted nothing: the account still holds no alias at all.
    let (status, body) = call(&router, "ListAccountAliases", &[]).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "member"),
        Vec::<String>::new(),
        "a refused alias must not be stored, got {:?}",
        elements(&body, "member")
    );
}

/// RULE: the refusal is about the FORM, so an alias inside it is stored and read back —
/// including at each published length bound and with the dashes the form does admit.
#[tokio::test]
async fn a_well_formed_account_alias_is_stored_and_read_back() {
    // Rows: the published minimum and maximum lengths, digits only, and single dashes at
    // one and at several positions. One account per row, because the alias is
    // account-wide and a create replaces it.
    for alias in [
        "abc".to_string(),
        format!("a{}c", "b".repeat(61)),
        "0123456789".to_string(),
        "a-1".to_string(),
        "my-account-alias".to_string(),
    ] {
        let router = router();
        let (status, body) = call(
            &router,
            "CreateAccountAlias",
            &[("AccountAlias", alias.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {alias:?}: expected HTTP 200, got {status}: {body}"
        );
        let (status, body) = call(&router, "ListAccountAliases", &[]).await;
        assert_eq!(
            status, 200,
            "case {alias:?}: expected HTTP 200, got {status}: {body}"
        );
        assert_eq!(
            elements(&body, "member"),
            vec![alias.clone()],
            "case {alias:?}: expected the stored alias to be reported, got {:?}",
            elements(&body, "member")
        );
        // The same alias is a name the delete accepts, so the form is read the same way on
        // both halves.
        let (status, body) = call(
            &router,
            "DeleteAccountAlias",
            &[("AccountAlias", alias.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {alias:?}: expected the stored alias to be deletable, got {status}: {body}"
        );
    }
}

/// RULE: a self-taken member's constraints are never stated as a clause — a request that
/// breaks one of them AND a member the model-level walk takes is answered by that walk,
/// over its own member's clauses alone.
#[tokio::test]
async fn a_request_breaking_both_layers_is_answered_by_the_model_level_walk() {
    let router = router();
    // `maxItemsType` is range 1..1000, which the model-level walk takes; the prefix is
    // outside `pathPrefixType`'s form, which IAM takes itself.
    let (status, body) = call(
        &router,
        "ListRoles",
        &[("PathPrefix", "eng"), ("MaxItems", "0")],
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        VALUE_REFUSAL_CODE,
        "expected the query protocol's refusal code, got {body}"
    );
    let message = element(&body, "Message");
    assert_eq!(
        message,
        "1 validation error detected: Value at 'maxItems' failed to satisfy constraint: \
         Member must have value greater than or equal to 1",
        "expected the page-size clause alone, got {message:?}"
    );
    assert!(
        !message.contains("pathPrefix") && !message.contains("eng"),
        "expected no clause about the self-taken member, got {message:?}"
    );
}
