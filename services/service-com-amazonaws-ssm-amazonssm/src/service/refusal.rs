// SPDX-License-Identifier: CC0-1.0

//! The WORDS of this service's boundary refusal: how the modelled constraints one request
//! broke become the `message` its `ValidationException` carries.
//!
//! [`crate::service`] owns the SEAMS — which refusal is answered where — and this module
//! owns the sentence, so one request cannot be refused in two voices.
//!
//! Almost all of that sentence is the framework's own, and deliberately so: the envelope,
//! the member paths, the requirement clauses and the value each clause quotes are rendered
//! by `aws_smithy_schema_server`, the one writer of that sentence. Three things about a
//! refusal of this service are NOT the framework's, and are stated here as rules over every
//! operation rather than as cases keyed on one recorded request:
//!
//! 1. THE ORDER an enumerated type's value set is listed in. A refusal of a value outside
//!    an enumerated type names the values that type admits, and this service lists them in
//!    the order its own hash table enumerates them — ascending string-hash bucket, with two
//!    values that share a bucket read later-declared first — which
//!    `service_common::validation_message::in_hashed_enum_order` is this emulator's one
//!    statement of. It is computed from the values themselves, so it belongs to the
//!    enumerated TYPE: every member of every operation carrying one enum is listed alike,
//!    and a type the model gains needs no row here. Recorded behaviour pins it on four of
//!    this service's types, two of which the model declares in another order
//!    (`InventoryQueryOperatorType`, declared `Equal, NotEqual, BeginWith, LessThan,
//!    GreaterThan, Exists`, is listed `NotEqual, BeginWith, Equal, LessThan, GreaterThan,
//!    Exists`).
//! 2. WHICH VALUES that set holds. The set a refusal lists is the set the live service
//!    ACCEPTS, which for three of this service's filter-key types is larger than this
//!    checkout's model declares — the model is behind the service, and a caller reads that
//!    list to pick a legal key. [`ACCEPTED_VALUE_SETS`] is that data, one row per type,
//!    recognised by the value set the violation carries so the row is found wherever the
//!    type appears. Which values a member ADMITS is NOT changed here, and cannot be from
//!    here: admission is the `@enum` constraint of the generated input schema, which
//!    codegen builds from `smithy-model/com-amazonaws-ssm-amazonssm.json` and the
//!    framework's own walk enforces before any handler or wrapper of this crate runs — so a
//!    key the checked-in model does not declare stays refused until that model is revised
//!    to the set the live service publishes. Until then the list a caller is handed is the
//!    one the live service hands back, which is what this rule owns; the admission half is
//!    the model's own update.
//! 3. WHETHER THE CLAUSE QUOTES THE VALUE. A clause of this service quotes the value it
//!    refused — `Value '<value>' at '<member path>' …` — except in the operations whose
//!    recorded refusals state none, which are listed in [`VALUELESS_OPERATIONS`] and report
//!    the member and the requirement alone (`Value at '<member path>' …`). The decision
//!    belongs to the OPERATION: the same `Contains` is withheld by the managed-node
//!    describe and quoted back by the inventory read, and in one of those operations a
//!    value outside an enumerated type, an absent member and a number above its maximum are
//!    all stated without a value.
//!
//! The fourth difference is not a wording rule but a reading one: the value a clause
//! stated at a COLLECTION member quotes is the collection the caller submitted, which no
//! violation carries — the framework's walk has only its size (`<n> items`), which is not a
//! value any caller sent. The collection comes from the echo the shared walk collects
//! beside each violation (`service_common::validation`), so
//! [`validation_refusal_quoting_collections`] is the entry point [`crate::service`]'s
//! per-operation wrapper words a refusal with, and
//! `service_common::validation_message::quoting_the_submitted_collection` is the one
//! statement of the restoring rule. Nothing else about a clause is restored: a value the
//! framework WITHHELD stays withheld, whatever the echo holds.

use ::aws_smithy_schema_server::constraints::Violation;
use ::service_common::validation::SubmittedEcho;
use ::service_common::validation_message::{
    in_hashed_enum_order, in_stated_enum_order, quoting_the_submitted_collection,
    StatedEnumValueSets,
};
use ::smithy_server_runtime::{Refusal, Refused};

/// The wire error code (`__type` / `x-amzn-errortype`) of this service's boundary refusal:
/// `ValidationException`, 400, which is also the code the framework's own default picks for
/// a broken modelled constraint — so a client branches on one code whichever layer refused
/// it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `com.amazonaws.ssm#CloudConnectorFilterKey` as this checkout's model declares it
/// (`smithy_query(query="shape",shape="CloudConnectorFilterKey")`), which is the set a
/// violation of a `CloudConnectorFilter$FilterKey` carries.
const CLOUD_CONNECTOR_FILTER_KEY_DECLARED: &[&str] = &["SubscriptionId", "TenantId"];

/// The `CloudConnectorFilterKey` values the live service accepts: the two the model
/// declares, then the two it does not (recorded behaviour — corpus `simparity-wire-aws-v1`,
/// fixture `ssm-cloudconnector-list-empty-filter` step `task#2`, where `ListCloudConnectors`
/// with an unknown `FilterKey` is refused with `[TenantId, OrganizationId, ProjectId,
/// SubscriptionId]`). Stated model-first so the listing order of two values that share a
/// hash bucket follows the declaration they are added to.
const CLOUD_CONNECTOR_FILTER_KEY_ACCEPTED: &[&str] = &[
    "SubscriptionId",
    "TenantId",
    "OrganizationId",
    "ProjectId",
];

/// `com.amazonaws.ssm#CommandFilterKey` as this checkout's model declares it
/// (`smithy_query(query="shape",shape="CommandFilterKey")`).
const COMMAND_FILTER_KEY_DECLARED: &[&str] = &[
    "InvokedAfter",
    "InvokedBefore",
    "Status",
    "ExecutionStage",
    "DocumentName",
];

/// The `CommandFilterKey` values the live service accepts: the five the model declares, then
/// the command and node ids it does not (recorded behaviour — fixture
/// `ssm-list-command-invocations-invalid-filter-key` step `task#1`, where
/// `ListCommandInvocations` with an unknown `key` is refused with `[CommandId, DocumentName,
/// ExecutionStage, InvokedBefore, InstanceId, InvokedAfter, Status]`).
const COMMAND_FILTER_KEY_ACCEPTED: &[&str] = &[
    "InvokedAfter",
    "InvokedBefore",
    "Status",
    "ExecutionStage",
    "DocumentName",
    "CommandId",
    "InstanceId",
];

/// `com.amazonaws.ssm#OpsItemFilterKey` as this checkout's model declares it
/// (`smithy_query(query="shape",shape="OpsItemFilterKey")`): 37 values.
const OPS_ITEM_FILTER_KEY_DECLARED: &[&str] = &[
    "Status",
    "CreatedBy",
    "Source",
    "Priority",
    "Title",
    "OpsItemId",
    "CreatedTime",
    "LastModifiedTime",
    "ActualStartTime",
    "ActualEndTime",
    "PlannedStartTime",
    "PlannedEndTime",
    "OperationalData",
    "OperationalDataKey",
    "OperationalDataValue",
    "ResourceId",
    "AutomationId",
    "Category",
    "Severity",
    "OpsItemType",
    "AccessRequestByRequesterArn",
    "AccessRequestByRequesterId",
    "AccessRequestByApproverArn",
    "AccessRequestByApproverId",
    "AccessRequestBySourceAccountId",
    "AccessRequestBySourceOpsItemId",
    "AccessRequestBySourceRegion",
    "AccessRequestByIsReplica",
    "AccessRequestByTargetResourceId",
    "ChangeRequestByRequesterArn",
    "ChangeRequestByRequesterName",
    "ChangeRequestByApproverArn",
    "ChangeRequestByApproverName",
    "ChangeRequestByTemplate",
    "ChangeRequestByTargetsResourceGroup",
    "InsightByType",
    "AccountId",
];

/// The `OpsItemFilterKey` values the live service accepts: the 37 the model declares, then
/// the five it does not — the Identity Center instance an access request is approved
/// through, the organization and organizational unit an OpsItem belongs to, and the two
/// halves of its assignee (recorded behaviour — fixture
/// `ssm-describe-ops-items-filter-paths` step `task#2`, where `DescribeOpsItems` with an
/// unknown `key` is refused with a 42-value set).
const OPS_ITEM_FILTER_KEY_ACCEPTED: &[&str] = &[
    "Status",
    "CreatedBy",
    "Source",
    "Priority",
    "Title",
    "OpsItemId",
    "CreatedTime",
    "LastModifiedTime",
    "ActualStartTime",
    "ActualEndTime",
    "PlannedStartTime",
    "PlannedEndTime",
    "OperationalData",
    "OperationalDataKey",
    "OperationalDataValue",
    "ResourceId",
    "AutomationId",
    "Category",
    "Severity",
    "OpsItemType",
    "AccessRequestByRequesterArn",
    "AccessRequestByRequesterId",
    "AccessRequestByApproverArn",
    "AccessRequestByApproverId",
    "AccessRequestBySourceAccountId",
    "AccessRequestBySourceOpsItemId",
    "AccessRequestBySourceRegion",
    "AccessRequestByIsReplica",
    "AccessRequestByTargetResourceId",
    "ChangeRequestByRequesterArn",
    "ChangeRequestByRequesterName",
    "ChangeRequestByApproverArn",
    "ChangeRequestByApproverName",
    "ChangeRequestByTemplate",
    "ChangeRequestByTargetsResourceGroup",
    "InsightByType",
    "AccountId",
    "IdcInstanceArn",
    "OrganizationId",
    "OrganizationalUnitId",
    "AssigneeType",
    "AssigneeId",
];

/// Every enumerated type of this service whose ACCEPTED value set is larger than this
/// checkout's model declares: the values as the model declares them, and the values the
/// service accepts.
///
/// A row is found by the value set the violation carries, matched as a SET
/// (`service_common::validation_message::stated_enum_value_set`), so one row covers every
/// member of every operation that targets that type. A type the model declares in full has
/// no row: its refusal lists exactly what the model declares.
const ACCEPTED_VALUE_SETS: &StatedEnumValueSets<'static> = &[
    (
        CLOUD_CONNECTOR_FILTER_KEY_DECLARED,
        CLOUD_CONNECTOR_FILTER_KEY_ACCEPTED,
    ),
    (COMMAND_FILTER_KEY_DECLARED, COMMAND_FILTER_KEY_ACCEPTED),
    (OPS_ITEM_FILTER_KEY_DECLARED, OPS_ITEM_FILTER_KEY_ACCEPTED),
];

/// The OPERATIONS whose boundary refusal states NO value — the member it was about and the
/// requirement it broke, and nothing of what the caller sent.
///
/// Recorded behaviour of this service, and recorded for every member and every constraint
/// class of these two:
///
/// * `DescribeSessions` — fixture
///   `ssm-sshp-traj-session-manager-ssh-document-and-state-mistakes`, step `task#3` (a
///   `State` outside `SessionState`) and step `task#4` (the same member absent), both
///   stated `Value at 'state' …`.
/// * `DescribeInstanceProperties` — fixture `ssm-w22n-traj-instance-properties-route`,
///   step `task#1` (an `Operator` outside its enumerated type, stated
///   `Value at 'filtersWithOperator.1.member.operator' …`) and step `task#6` (a
///   `MaxResults` above its `@range` maximum, stated `Value at 'maxResults' …`).
///
/// So the decision belongs to the OPERATION rather than to one member, one list position
/// or one constraint class — and every OTHER operation quotes the value the framework
/// recorded: the same `Contains` is withheld by the managed-node describe and quoted back
/// by `GetInventory` (fixture `ssm-w22n-traj-inventory-filter-mistakes`), and the keys
/// `ListCloudConnectors`, `ListCommandInvocations` and `DescribeOpsItems` refuse are quoted
/// too.
const VALUELESS_OPERATIONS: &[&str] = &["DescribeSessions", "DescribeInstanceProperties"];

/// Whether the clauses of a refusal of `operation` state no value: it is one of
/// [`VALUELESS_OPERATIONS`]. `None` — a refusal that named no modelled operation at all —
/// quotes the value, which is what every operation but those two does.
fn states_no_value(operation: Option<&str>) -> bool {
    operation.is_some_and(|operation| VALUELESS_OPERATIONS.contains(&operation))
}

/// `violation` as this service states it: the accepted value set in place of the declared
/// one (rule 2), listed in this service's order (rule 1), with the value half dropped where
/// the operation states none (rule 3).
///
/// The CLAUSE is still the framework's — only what goes into it moves — so the member path,
/// the requirement's wording and the punctuation around them keep coming from the one place
/// that writes them. A valueless clause is the framework's own rendering of a violation
/// whose value a response does not carry, rather than a second sentence spelled here.
fn stated(violation: &Violation, valueless: bool) -> Violation {
    let accepted = in_stated_enum_order(violation, ACCEPTED_VALUE_SETS);
    let listed = in_hashed_enum_order(&accepted);
    if valueless {
        return Violation {
            value: None,
            sensitive: true,
            ..listed
        };
    }
    listed
}

/// How this service states a set of broken modelled constraints: its `ValidationException`,
/// carrying the framework's envelope over the clauses of the violations as this service
/// states their contents. `operation` is the modelled operation the request named, or
/// `None` when it named none.
///
/// `None` when there is nothing to state, which keeps the framework's own wording rather
/// than answering with an empty message.
pub(super) fn validation_refusal(
    operation: Option<&str>,
    violations: &[Violation],
) -> Option<Refused> {
    let valueless = states_no_value(operation);
    let stated: Vec<Violation> = violations
        .iter()
        .map(|violation| stated(violation, valueless))
        .collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: ::aws_smithy_schema_server::envelope(&stated)?,
    })
}

/// How this service states a set of broken modelled constraints, for a caller holding the
/// ECHO of each refused value beside the violation: the clause stated at a COLLECTION
/// member quotes the collection the caller submitted, where the framework's walk stored its
/// own summary of one, and every other clause is exactly [`validation_refusal`]'s.
pub(super) fn validation_refusal_quoting_collections(
    operation: &str,
    refused: Vec<(Violation, SubmittedEcho)>,
) -> Option<Refused> {
    let quoting: Vec<Violation> = refused
        .into_iter()
        .map(|(violation, echo)| quoting_the_submitted_collection(violation, echo))
        .collect();
    validation_refusal(Some(operation), &quoting)
}

/// How this service states one framework-level [`Refusal`], or `None` to keep the
/// framework's own wording — the whole decision both refusal paths of
/// [`crate::service`] go through, so the two cannot answer one request differently.
///
/// This path has no modelled operation to read the request against (one that named a
/// modelled operation is answered by the per-operation wrapper, which also knows WHICH
/// operation), so its clauses quote the value — what every operation but the two of
/// [`VALUELESS_OPERATIONS`] does.
///
/// A caller the framework would not authenticate keeps the framework's 403 and its wording:
/// this service's boundary wording is about the REQUEST's values, and nothing recorded here
/// words that denial differently.
pub(super) fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => validation_refusal(None, violations),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! The wording rules, driven on violations built by hand. The wire behaviour they add
    //! up to — which operations are refused this way, with which status and code — is
    //! `tests/all/boundary_refusal_wording.rs`.

    use super::*;
    use ::aws_smithy_schema_server::constraints::ViolationKind;

    /// `com.amazonaws.ssm#SessionState`
    /// (`smithy_query(query="shape",shape="SessionState")`), written out here rather than
    /// read from the service so the test pins the listing instead of echoing it.
    const SESSION_STATE: &[&str] = &["Active", "History"];

    /// `com.amazonaws.ssm#InstancePropertyFilterOperator`
    /// (`smithy_query(query="shape",shape="InstancePropertyFilterOperator")`).
    const INSTANCE_PROPERTY_FILTER_OPERATOR: &[&str] =
        &["Equal", "NotEqual", "BeginWith", "LessThan", "GreaterThan"];

    /// `com.amazonaws.ssm#InventoryQueryOperatorType`
    /// (`smithy_query(query="shape",shape="InventoryQueryOperatorType")`).
    const INVENTORY_QUERY_OPERATOR_TYPE: &[&str] = &[
        "Equal",
        "NotEqual",
        "BeginWith",
        "LessThan",
        "GreaterThan",
        "Exists",
    ];

    /// A violation of `kind` on `path`, carrying `value` as the framework's walk would.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(ToString::to_string),
            sensitive: false,
        }
    }

    /// An enum violation of the type `declared`, with `value` refused at `path`.
    fn enum_violation(path: &str, declared: &[&str], value: &str) -> Violation {
        violation(
            path,
            ViolationKind::EnumValue(declared.iter().map(ToString::to_string).collect()),
            Some(value),
        )
    }

    /// The message of the refusal `violations` are stated as, for a request that named no
    /// operation of [`VALUELESS_OPERATIONS`].
    fn message(violations: &[Violation]) -> String {
        message_for(None, violations)
    }

    /// The message of the refusal `violations` are stated as, for a request that named
    /// `operation`.
    fn message_for(operation: Option<&str>, violations: &[Violation]) -> String {
        validation_refusal(operation, violations)
            .expect("violations to state are stated")
            .message
    }

    /// The value set one enum violation's clause lists, as text.
    fn listed_set(violation: &Violation) -> String {
        let stated = message(std::slice::from_ref(violation));
        let (_, set) = stated
            .split_once("enum value set: [")
            .unwrap_or_else(|| panic!("case {stated:?}: the clause lists no enum value set"));
        set.trim_end_matches(']').to_string()
    }

    /// RULE 1 and RULE 2 over every type recorded behaviour pins a listing of: the set is
    /// the one the service ACCEPTS, in the order the service lists it, whatever order the
    /// model declares its values in.
    #[test]
    fn an_enum_value_set_is_listed_as_the_service_lists_it() {
        // (case, the values a violation carries, the set the clause lists)
        let cases: &[(&str, &[&str], &str)] = &[
            (
                "a filter key the model declares fewer values than the service accepts",
                CLOUD_CONNECTOR_FILTER_KEY_DECLARED,
                "TenantId, OrganizationId, ProjectId, SubscriptionId",
            ),
            (
                "a filter key whose accepted set adds two ids",
                COMMAND_FILTER_KEY_DECLARED,
                "CommandId, DocumentName, ExecutionStage, InvokedBefore, InstanceId, \
                 InvokedAfter, Status",
            ),
            (
                "an operator type the model declares in full, listed in another order",
                INVENTORY_QUERY_OPERATOR_TYPE,
                "NotEqual, BeginWith, Equal, LessThan, GreaterThan, Exists",
            ),
            (
                "the five-value operator type the same order holds over",
                INSTANCE_PROPERTY_FILTER_OPERATOR,
                "NotEqual, BeginWith, Equal, LessThan, GreaterThan",
            ),
            (
                "a two-value type the service lists as the model declares it",
                SESSION_STATE,
                "Active, History",
            ),
            (
                // `com.amazonaws.ssm#ParameterType`
                // (`smithy_query(query="shape",shape="ParameterType")`), a type recorded
                // behaviour pins no listing of: rule 1 holds over it all the same, because
                // the order is computed from the values and not read from a row.
                "a type with no row of its own is listed in the service's order too",
                &["String", "StringList", "SecureString"],
                "SecureString, StringList, String",
            ),
        ];
        for (case, declared, expected) in cases {
            let listed = listed_set(&enum_violation("key", declared, "NotAValue"));
            assert_eq!(
                &listed, expected,
                "case {case:?}: expected [{expected}], got [{listed}]"
            );
        }
    }

    /// RULE 3: the two operations whose clauses state no value state none for every member
    /// and every constraint class, and every other operation quotes the value the caller
    /// sent.
    #[test]
    fn only_the_valueless_operations_withhold_the_refused_value() {
        // (case, the operation the request named, the violation, the subject half its
        // clause opens with)
        let cases: &[(&str, &str, Violation, &str)] = &[
            (
                "a session state outside its enumerated type",
                "DescribeSessions",
                enum_violation("state", SESSION_STATE, "active"),
                "Value at 'state'",
            ),
            (
                "the same member absent altogether",
                "DescribeSessions",
                violation("state", ViolationKind::MissingRequiredMember, None),
                "Value at 'state'",
            ),
            (
                "a page size over the same operation's maximum",
                "DescribeSessions",
                violation(
                    "maxResults",
                    ViolationKind::MaxValue("200".to_string()),
                    Some("201"),
                ),
                "Value at 'maxResults'",
            ),
            (
                "a managed-node filter operator in the first list element",
                "DescribeInstanceProperties",
                enum_violation(
                    "filtersWithOperator.1.member.operator",
                    INSTANCE_PROPERTY_FILTER_OPERATOR,
                    "Contains",
                ),
                "Value at 'filtersWithOperator.1.member.operator'",
            ),
            (
                "the same member in a later list element",
                "DescribeInstanceProperties",
                enum_violation(
                    "filtersWithOperator.7.member.operator",
                    INSTANCE_PROPERTY_FILTER_OPERATOR,
                    "Contains",
                ),
                "Value at 'filtersWithOperator.7.member.operator'",
            ),
            (
                "a page size over that operation's maximum",
                "DescribeInstanceProperties",
                violation(
                    "maxResults",
                    ViolationKind::MaxValue("1000".to_string()),
                    Some("1001"),
                ),
                "Value at 'maxResults'",
            ),
            (
                "an inventory filter type, which quotes the same value",
                "GetInventory",
                enum_violation(
                    "filters.1.member.type",
                    INVENTORY_QUERY_OPERATOR_TYPE,
                    "Contains",
                ),
                "Value 'Contains' at 'filters.1.member.type'",
            ),
            (
                "a cloud-connector filter key",
                "ListCloudConnectors",
                enum_violation(
                    "filters.1.member.filterKey",
                    CLOUD_CONNECTOR_FILTER_KEY_DECLARED,
                    "NotARealFilterKey",
                ),
                "Value 'NotARealFilterKey' at 'filters.1.member.filterKey'",
            ),
            (
                "a page size over another operation's maximum",
                "GetInventory",
                violation(
                    "maxResults",
                    ViolationKind::MaxValue("50".to_string()),
                    Some("51"),
                ),
                "Value '51' at 'maxResults'",
            ),
        ];
        for (case, operation, broken, expected_subject) in cases {
            let stated = message_for(Some(operation), std::slice::from_ref(broken));
            assert!(
                stated.contains(expected_subject),
                "case {case:?}: expected a clause opening {expected_subject:?}, got {stated:?}"
            );
        }
    }

    /// Every other violation keeps the framework's clause whole — including the value a
    /// clause stated at a COLLECTION member quotes, which is the collection the caller
    /// submitted.
    #[test]
    fn every_other_violation_keeps_the_frameworks_clause() {
        let cases: &[(&str, Violation)] = &[
            (
                "a list over its length maximum, quoting the list",
                violation(
                    "names",
                    ViolationKind::MaxLength(10),
                    Some("[/a, /b, /c]"),
                ),
            ),
            (
                "an absent required member",
                violation("name", ViolationKind::MissingRequiredMember, None),
            ),
            (
                "a value outside a pattern",
                violation(
                    "instanceId",
                    ViolationKind::Pattern("^i-\\w{8}$".to_string()),
                    Some("not a node"),
                ),
            ),
            (
                "a number over its range maximum",
                violation(
                    "maxResults",
                    ViolationKind::MaxValue("50".to_string()),
                    Some("51"),
                ),
            ),
        ];
        for (case, broken) in cases {
            let stated = message(std::slice::from_ref(broken));
            let expected = format!("1 validation error detected: {}", broken.message());
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// Two broken constraints are the framework's plural envelope over both clauses, each
    /// stated by the rules above.
    #[test]
    fn two_violations_are_stated_under_the_plural_envelope() {
        let page = violation(
            "maxResults",
            ViolationKind::MinValue("1".to_string()),
            Some("0"),
        );
        let state = enum_violation("state", SESSION_STATE, "active");
        let stated = message_for(Some("DescribeSessions"), &[page, state]);
        let expected = "2 validation errors detected: Value at 'maxResults' failed to satisfy \
                        constraint: Member must have value greater than or equal to 1; Value \
                        at 'state' failed to satisfy constraint: Member must satisfy enum \
                        value set: [Active, History]";
        assert_eq!(
            stated, expected,
            "case a page size and a session state: expected {expected:?}, got {stated:?}"
        );
    }

    /// Nothing to state is no refusal of this kind at all, rather than an empty message.
    #[test]
    fn no_violations_state_no_refusal() {
        assert!(
            validation_refusal(None, &[]).is_none(),
            "case no violations: expected no refusal to be worded"
        );
        assert!(
            stated_refusal(&Refusal::Unauthenticated).is_none(),
            "case an unauthenticated caller: expected the framework's own wording"
        );
    }

    /// Each accepted row holds every value the model declares for that type, so rule 2 only
    /// ever ADDS to the set a caller is told about.
    #[test]
    fn an_accepted_set_holds_every_declared_value() {
        for (declared, accepted) in ACCEPTED_VALUE_SETS {
            for value in *declared {
                assert!(
                    accepted.contains(value),
                    "case {value:?}: the accepted set {accepted:?} drops a value the model \
                     declares"
                );
            }
            assert!(
                accepted.len() > declared.len(),
                "case {declared:?}: a row that adds nothing needs no row at all"
            );
        }
    }
}
