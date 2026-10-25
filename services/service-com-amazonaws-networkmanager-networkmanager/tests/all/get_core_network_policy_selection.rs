// SPDX-License-Identifier: CC0-1.0

//! Which version of a core network's policy a READ addresses, driven end-to-end over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: `GetCoreNetworkPolicy` answers exactly the version the request
//! addresses — the `PolicyVersionId` it names, else the version its `Alias` selects
//! (`LATEST` the newest version the core network still holds, `LIVE` and an absent alias
//! the version in force) — reported from the stored history and changing nothing; a core
//! network, or a version of it, that the store does not hold is a not-found refusal naming
//! that resource, and a selector outside the model never reaches the history at all.
//!
//! The tables below span that class: histories differing in how many versions they hold,
//! every selector a reader can address a version by, version ids on either side of the
//! history (including both ends and a never-minted one), core network ids at each bound
//! the modelled id pattern publishes and one past each, and aliases outside the modelled
//! enumeration. The history-wide behaviour of a put, a delete and a restore belongs to
//! `crate::core_network_policy_versions`, `crate::core_network_policy_version_listing` and
//! `crate::core_network_policy_version_restore`; nothing here reaches into `crate::state`.

use serde_json::{json, Value};

use crate::core_network_policy_versions::{
    append_versions, core_network_with_policy, document_of_version, get_policy, policy, policy_uri,
};
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::resource_not_found_message::assert_not_found_answer;

/// The `ResourceType` a refusal reports for a core network and for one of its policy
/// versions: the ARN resource types this service holds.
const CORE_NETWORK: &str = "core-network";
/// See [`CORE_NETWORK`].
const CORE_NETWORK_POLICY: &str = "core-network-policy";

/// The alias the version `version_id` of a history of `held` versions carries: `LIVE` for
/// the version in force (version 1, the document the create executed), `LATEST` for the
/// newest version held when that is not the one in force, and NONE for every version in
/// between — a version the history has moved past carries no alias.
fn expected_alias(version_id: i32, held: i32) -> Value {
    if version_id == 1 {
        json!("LIVE")
    } else if version_id == held {
        json!("LATEST")
    } else {
        Value::Null
    }
}

/// The `Description` the version `version_id` of a history carries: the create stores none
/// with version 1, and `crate::core_network_policy_versions::append_versions` describes
/// every version it appends by its own id.
fn expected_description(version_id: i32) -> Value {
    if version_id == 1 {
        Value::Null
    } else {
        json!(format!("version {version_id}"))
    }
}

/// Whether the refusal `body` names `member` as the member at fault — in its prose, which
/// is where a constraint failure reports the offending member's path, or in the
/// `ValidationException$Fields` entry for it. Which of the two carries it is the framework's
/// to decide, so the assertion pins only that the member is named.
fn refusal_names(body: &Value, member: &str) -> bool {
    if body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .is_some_and(|message| message.contains(member))
    {
        return true;
    }
    body["Fields"]
        .as_array()
        .is_some_and(|fields| fields.iter().any(|field| field["Name"] == json!(member)))
}

/// One row of the history table: a core network holding `held` policy versions, ids 1..held.
struct HistoryCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// How many versions the history holds, the live version 1 included.
    held: i32,
}

#[tokio::test]
async fn every_selector_answers_the_version_it_addresses() {
    let emulator = Emulator::new();
    // The rows differ in how long the history is: only the version in force, one appended
    // on top of it (so LIVE and LATEST are adjacent), and a longer history that has
    // versions in between carrying neither alias.
    let cases = [
        HistoryCase {
            name: "only the version in force",
            held: 1,
        },
        HistoryCase {
            name: "one version appended on top of the live one",
            held: 2,
        },
        HistoryCase {
            name: "a history with versions between the live and the newest",
            held: 5,
        },
    ];

    for case in &cases {
        let core_network_id =
            core_network_with_policy(&emulator, case.name, &document_of_version(1)).await;
        let minted = append_versions(&emulator, case.name, &core_network_id, case.held - 1).await;
        let expected_minted: Vec<i32> = (2..=case.held).collect();
        assert_eq!(
            minted, expected_minted,
            "case {:?}: the history must hold ids 1..{}, got 1 plus {minted:?}",
            case.name, case.held
        );

        // Every change set this history owes a transition has settled by now, so the
        // `ChangeSetState` the reads below report is terminal and two reads of one selector
        // are comparable in full rather than member by member.
        tokio::time::sleep(PAST_SETTLE).await;

        // Every selector a reader can address a version by, and the version id it
        // addresses: no selector and `LIVE` the version in force, `LATEST` the newest
        // version held, and each version id in the history itself.
        let mut selectors: Vec<(String, i32)> = vec![
            (String::new(), 1),
            ("?alias=LIVE".to_string(), 1),
            ("?alias=LATEST".to_string(), case.held),
        ];
        selectors.extend(
            (1..=case.held).map(|version_id| (format!("?policyVersionId={version_id}"), version_id)),
        );

        for (query, expected_version) in &selectors {
            let (status, read) = get_policy(&emulator, &core_network_id, query).await;
            assert_eq!(
                status, 200,
                "case {:?}: GetCoreNetworkPolicy({query:?}) expected 200, got {status} with {read}",
                case.name
            );
            let answered = policy(&read);
            assert_eq!(
                answered["PolicyVersionId"],
                json!(expected_version),
                "case {:?}: {query:?} must address version {expected_version}, got {read}",
                case.name
            );
            assert_eq!(
                answered["PolicyDocument"],
                json!(document_of_version(*expected_version)),
                "case {:?}: {query:?} must answer the document stored with version {expected_version}, got {read}",
                case.name
            );
            assert_eq!(
                answered["Alias"],
                expected_alias(*expected_version, case.held),
                "case {:?}: {query:?} answered version {expected_version} with the wrong alias in {read}",
                case.name
            );
            assert_eq!(
                answered["Description"],
                expected_description(*expected_version),
                "case {:?}: {query:?} must answer the description stored with version {expected_version}, got {read}",
                case.name
            );
            assert_eq!(
                answered["CoreNetworkId"],
                json!(core_network_id),
                "case {:?}: {query:?} must name the core network holding the version, got {read}",
                case.name
            );

            // A read reports the history; it never advances it, so the same selector
            // answers the same version to the next caller.
            let (status, again) = get_policy(&emulator, &core_network_id, query).await;
            assert_eq!(
                status, 200,
                "case {:?}: a second GetCoreNetworkPolicy({query:?}) expected 200, got {status} with {again}",
                case.name
            );
            assert_eq!(
                read, again,
                "case {:?}: {query:?} must be a pure read, got {read} then {again}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_read_of_a_resource_the_store_does_not_hold_is_a_not_found() {
    let emulator = Emulator::new();
    let core_network_id =
        core_network_with_policy(&emulator, "unknown selectors", &document_of_version(1)).await;
    append_versions(&emulator, "unknown selectors", &core_network_id, 2).await;

    // Version ids this core network never minted, on either side of the history it holds
    // (1..3): the id below its first version, a negative id, the id the next put would
    // take, and one far past the history.
    for (case, version_id) in [
        ("the id below the first version", 0),
        ("a negative version id", -1),
        ("the id the next put would mint", 4),
        ("an id far past the history", 97),
    ] {
        assert_not_found_answer(
            &emulator,
            case,
            "GET",
            &policy_uri(&core_network_id, &format!("?policyVersionId={version_id}")),
            None,
            &format!("{core_network_id}/{version_id}"),
            CORE_NETWORK_POLICY,
        )
        .await;
    }

    // A core network id this service never minted: the refusal names the CORE NETWORK,
    // not a policy, whichever selector the request used, because the version is only
    // looked for inside a core network the store holds. Both rows satisfy the modelled
    // `CoreNetworkId` pattern `^core-network-([0-9a-f]{8,17})$` (`smithy_query
    // shape=CoreNetworkId`) — at each width it publishes — so both reach the handler.
    for (case, unknown) in [
        ("the 8-hex lower bound of the id pattern", "core-network-0123abcd"),
        (
            "the 17-hex upper bound of the id pattern",
            "core-network-0123456789abcdef0",
        ),
    ] {
        for query in ["", "?alias=LIVE", "?alias=LATEST", "?policyVersionId=1"] {
            assert_not_found_answer(
                &emulator,
                &format!("{case} read with {query:?}"),
                "GET",
                &policy_uri(unknown, query),
                None,
                unknown,
                CORE_NETWORK,
            )
            .await;
        }
    }

    // None of those refusals touched the history they addressed.
    let (status, live) = get_policy(&emulator, &core_network_id, "").await;
    assert_eq!(
        status, 200,
        "the refused reads must leave the live version readable, got {status} with {live}"
    );
    assert_eq!(
        policy(&live)["PolicyVersionId"],
        json!(1),
        "the refused reads must leave the version in force where it was, got {live}"
    );
    let (status, latest) = get_policy(&emulator, &core_network_id, "?alias=LATEST").await;
    assert_eq!(
        status, 200,
        "the refused reads must leave the newest version readable, got {status} with {latest}"
    );
    assert_eq!(
        policy(&latest)["PolicyVersionId"],
        json!(3),
        "the refused reads must leave the history as long as it was, got {latest}"
    );
}

#[tokio::test]
async fn a_selector_outside_the_model_is_refused_before_any_version_is_read() {
    let emulator = Emulator::new();
    let core_network_id =
        core_network_with_policy(&emulator, "bad selectors", &document_of_version(1)).await;

    // A core network id outside the modelled `CoreNetworkId` pattern
    // `^core-network-([0-9a-f]{8,17})$` (`smithy_query shape=CoreNetworkId`): one past each
    // width it publishes, and one whose digits are outside its alphabet. None of them can
    // name a resource, so the constraint walk refuses the request rather than reporting a
    // missing one.
    for (case, malformed) in [
        ("one hex digit short of the pattern", "core-network-0123abc"),
        (
            "one hex digit past the pattern",
            "core-network-0123456789abcdef01",
        ),
        ("digits outside the pattern's alphabet", "core-network-zzzzzzzz"),
    ] {
        let (status, refused) = get_policy(&emulator, malformed, "").await;
        assert_eq!(
            status, 400,
            "case {case:?}: an id outside the modelled pattern must be refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: the refusal must be a validation failure, got {refused}"
        );
        assert!(
            refusal_names(&refused, "coreNetworkId"),
            "case {case:?}: the refusal must name the member at fault, got {refused}"
        );
    }

    // An alias outside the modelled `CoreNetworkPolicyAlias` enumeration (`LATEST |
    // LIVE`, `smithy_query shape=CoreNetworkPolicyAlias`), which is case-sensitive: a
    // value it does not list selects no version and is refused, never answered with some
    // other version.
    for (case, alias) in [
        ("the live alias in lower case", "live"),
        ("the latest alias in mixed case", "Latest"),
        ("an alias the enumeration does not list", "PREVIOUS"),
    ] {
        let (status, refused) =
            get_policy(&emulator, &core_network_id, &format!("?alias={alias}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unmodelled alias must be refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: the refusal must be a validation failure, got {refused}"
        );
        assert!(
            refusal_names(&refused, "alias"),
            "case {case:?}: the refusal must name the member at fault, got {refused}"
        );
    }

    // The refusals left the one version this core network holds readable.
    let (status, live) = get_policy(&emulator, &core_network_id, "?alias=LIVE").await;
    assert_eq!(
        status, 200,
        "the refused selectors must leave the live version readable, got {status} with {live}"
    );
    assert_eq!(
        policy(&live)["PolicyDocument"],
        json!(document_of_version(1)),
        "the refused selectors must leave the live document untouched, got {live}"
    );
}
