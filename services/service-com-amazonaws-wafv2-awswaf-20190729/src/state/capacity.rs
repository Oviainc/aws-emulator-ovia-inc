// SPDX-License-Identifier: CC0-1.0

//! The WCU cost model: what one rule list costs to run.

use super::*;

/// The web ACL capacity units (WCUs) a rule list consumes.
///
/// `WebACL$Capacity` is "the web ACL capacity units (WCUs) currently being used by
/// this web ACL", so it is DERIVED from the stored rules rather than carried in
/// the request, and "WAF calculates capacity differently for each rule type, to
/// reflect the relative cost of each rule" (`CheckCapacity`'s own doc) — so the
/// figure is a function of the statement KINDS in the list and of the modifiers each
/// one carries, never of how many rules there are. A logical statement costs the sum
/// of what it nests.
///
/// Each kind's base cost is a constant below. On top of a base, the modifiers a
/// statement carries are charged: the component it inspects can add a surcharge
/// ([`MULTI_VALUED_COMPONENT_WCU`]) or multiply the match itself
/// ([`JSON_BODY_BASE_MULTIPLIER`]), and each text transformation it applies adds a
/// flat [`TEXT_TRANSFORMATION_WCU`] — once per (component, transformation chain)
/// across the WHOLE rule list, because the transformed content is computed once and
/// every statement over that component inspects it (see [`CapacityAccount`]).
///
/// One cost is not a constant: a `RuleGroupReferenceStatement` costs the
/// referenced rule group's own `Capacity` — "the web ACL capacity units (WCUs)
/// required for this rule group", fixed when the group is created — so
/// `rule_group_capacities` carries `ARN` → `Capacity` for the scope being saved
/// into, as [`ComAmazonawsWafv2Awswaf20190729State::rule_group_capacities`] reads
/// it out of state. A reference naming a rule group this service does not hold is
/// refused on ingress by
/// [`ComAmazonawsWafv2Awswaf20190729State::unresolvable_rule_reference`], so every
/// reference a stored rule list carries is in that map.
///
/// A `ManagedRuleGroupStatement` is priced the same way, from the published
/// capacity of the catalogued group it names ([`managed_rule_group`]): a managed
/// rule group's capacity is a property of the group, not of the request, and a
/// statement naming a group the catalogue does not list is refused on ingress by
/// the same resolution, so this function never has to price a statement whose cost
/// it cannot know and no caller is handed an understated `Capacity`.
///
/// Every term is added with SATURATING arithmetic: one of them — a referenced rule
/// group's declared `Capacity` — is caller-supplied and modelled `range 1..`, so a
/// rule list referencing enough expensive groups could drive the sum past
/// `i64::MAX`. Saturating at the modelled maximum answers a capacity that is simply
/// above any limit, where wrapping would answer a NEGATIVE capacity (which
/// `ConsumedCapacity` is `range 0..`) and overflow-checked arithmetic would panic on
/// a request.
///
/// What this reports decides a SAVE as well as a report: `CheckCapacity` publishes
/// it, and `CreateRuleGroup` refuses a rule list that needs more than the
/// `Capacity` the request declares ("WAF enforces this limit"). So a figure below
/// publishes an understated price, and a figure ABOVE the published cost of its
/// statement kind would refuse a save WAF accepts — re-price and re-read that
/// refusal together.
pub fn web_acl_capacity(rules: &[Rule], rule_group_capacities: &HashMap<String, i64>) -> i64 {
    let mut account = CapacityAccount::default();
    for rule in rules {
        account.add_statement(
            &rule.statement,
            MAX_STATEMENT_NESTING_DEPTH,
            rule_group_capacities,
        );
    }
    account.total()
}

/// The WCUs one `ByteMatchStatement` consumes before its modifiers, for a
/// positional constraint that pins the search to one place in the component.
///
/// `ByteMatchStatement$PositionalConstraint` splits the search into the ones that
/// compare the component against `SearchString` at a FIXED position — "the value of
/// the specified part of the web request must exactly match", "must appear at the
/// beginning", "must appear at the end" — and the ones that have to scan it for the
/// string anywhere in it ([`BYTE_MATCH_SCANNING_WCU`]). A fixed-position compare is
/// the cheaper of the two.
pub const BYTE_MATCH_ANCHORED_WCU: i64 = 2;

/// The WCUs one `ByteMatchStatement` consumes before its modifiers, for a
/// positional constraint that has to SCAN the component.
///
/// `CONTAINS` is "must include the value of `SearchString`, but the location doesn't
/// matter" and `CONTAINS_WORD` adds the word-boundary conditions around that
/// inclusion, so both search the whole component rather than comparing it at one
/// position. Pinned by the recorded corpus: a web ACL whose three rules each hold
/// one such `CONTAINS` byte match over a single field reports `Capacity` 30.
pub const BYTE_MATCH_SCANNING_WCU: i64 = 10;

/// The WCUs one `SqliMatchStatement` consumes before its modifiers at the
/// sensitivity its member documents as the default.
///
/// `SqliMatchStatement$SensitivityLevel` documents `LOW` as "generally a better
/// choice for resources that already have other protections", with "Default: `LOW`",
/// and `HIGH` as the level that "detects more attacks" by inspecting for more of
/// them ([`SQLI_MATCH_HIGH_WCU`]) — so the cheaper level is also the one a statement
/// that names none is priced at.
pub const SQLI_MATCH_LOW_WCU: i64 = 20;

/// The WCUs one `SqliMatchStatement` at `HIGH` sensitivity consumes before its
/// modifiers. See [`SQLI_MATCH_LOW_WCU`].
pub const SQLI_MATCH_HIGH_WCU: i64 = 30;

/// The WCUs one `XssMatchStatement` consumes before its modifiers.
pub const XSS_MATCH_WCU: i64 = 40;

/// The WCUs one `RegexMatchStatement` consumes before its modifiers.
pub const REGEX_MATCH_WCU: i64 = 3;

/// The WCUs one `RegexPatternSetReferenceStatement` consumes before its modifiers:
/// a statement that runs a whole set of expressions over the component rather than
/// the one expression a `RegexMatchStatement` carries.
pub const REGEX_PATTERN_SET_REFERENCE_WCU: i64 = 25;

/// The WCUs one of the statements that inspects a request ATTRIBUTE consumes: a
/// size constraint, a geo match, an IP set reference, a label match or an ASN
/// match. Each of them decides from one already-extracted value, which is the
/// cheapest thing a rule can do.
pub const ATTRIBUTE_MATCH_WCU: i64 = 1;

/// The WCUs one `RateBasedStatement` consumes before its custom keys and anything
/// it nests.
///
/// Pinned by the recorded corpus: a web ACL holding exactly one rate-based rule
/// that aggregates on `IP`, with no scope-down statement and no custom keys,
/// reports `Capacity` 2.
pub const RATE_BASED_WCU: i64 = 2;

/// The WCUs a `RateBasedStatement` adds for EACH custom aggregation key.
///
/// Aggregating on request components costs more than aggregating on the source IP,
/// because WAF has to extract and hash a component of every request per key:
/// `RateBasedStatement$AggregateKeyType` documents `CUSTOM_KEYS` as "aggregate the
/// request counts using one or more web request components as the aggregate keys",
/// and "each unique set of values for the aggregation keys that you specify is a
/// separate aggregation instance, with the value from each key contributing to the
/// aggregation instance definition" — so every key in `CustomKeys`
/// (`length 0..5`) is extracted and is charged. The recorded corpus pins one key at
/// `Capacity` 32, [`RATE_BASED_WCU`] plus this.
pub const RATE_BASED_CUSTOM_KEY_WCU: i64 = 30;

/// The WCUs a statement adds for each text transformation it applies to the
/// component it inspects.
///
/// `TextTransformation` is what "WAF performs … on the specified content, starting
/// from the lowest priority setting", before the statement inspects it, so it is
/// work on top of the match itself. The `NONE` transformation transforms nothing
/// and adds nothing.
pub const TEXT_TRANSFORMATION_WCU: i64 = 10;

/// The `TextTransformationType` that transforms nothing
/// (`smithy_query(query="shape",shape="TextTransformationType")`).
pub const TEXT_TRANSFORMATION_NONE: &str = "NONE";

/// The WCUs a statement adds for inspecting EVERY value of a multi-valued request
/// component rather than one named value.
///
/// `FieldToMatch$AllQueryArguments` is "inspect all query arguments",
/// `FieldToMatch$Headers` "inspect the request headers" and
/// `FieldToMatch$Cookies` "inspect the request cookies" — each of the three makes
/// WAF split one component into its entries and run the match over the entries the
/// `MatchPattern` selects, instead of over one already-extracted value. The three
/// are modelled alike for exactly that reason (the latter two carry a
/// `MatchPattern`, a `MatchScope` and an `OversizeHandling` of their own), so they
/// are charged alike, which is why this names the work rather than the one
/// component the figure was first pinned on.
pub const MULTI_VALUED_COMPONENT_WCU: i64 = 10;

/// What inspecting the request body AS JSON multiplies a statement's base cost by.
///
/// `FieldToMatch$JsonBody` is "inspect the request body as JSON", which makes WAF
/// parse the body and then run the match over the elements the `MatchPattern`
/// selects, rather than over one value — so the match itself is done repeatedly and
/// the base cost of the statement is doubled.
pub const JSON_BODY_BASE_MULTIPLIER: i64 = 2;

/// The most (component, transformation chain) pairs one accounting shares a charge
/// between.
///
/// Unlike the per-scope quotas above this is NOT a figure WAF publishes: it is this
/// service's own bound on the memory one request's pricing may take, since a rule
/// list is caller-supplied and nothing in the model caps how many transformations it
/// carries in total. Past the bound the sharing simply stops and each further
/// transformation is charged on its own, so the figure can only RISE — a rule list
/// this big already costs more WCUs than any rule group or web ACL could be sized
/// for, so no save could fit it either way.
pub const MAX_SHARED_TRANSFORMATION_CHARGES: usize = 10_000;

/// The WCU accounting of one rule list, as it is walked.
///
/// The per-statement base costs accumulate in `base`. The text-transformation
/// charge does NOT: a transformation is applied to a request component once and the
/// transformed content is what every statement over that component then inspects,
/// so the charge belongs to the (component, transformation chain) pair and is paid
/// ONCE for the whole rule list however many rules apply it. `transformations`
/// therefore collects those pairs and [`Self::total`] charges each of them once.
///
/// A pair is held as the 64-bit digest of its (component, chain), so what one
/// request's pricing allocates is one machine word per transformation it carries
/// rather than a copy of every chain prefix — and never more than
/// [`MAX_SHARED_TRANSFORMATION_CHARGES`] of them.
///
/// Every term is added with saturating arithmetic; see [`web_acl_capacity`].
#[derive(Debug, Default)]
pub(super) struct CapacityAccount {
    /// The per-statement base costs, with each statement's own modifiers applied,
    /// plus any transformation charged on its own past the sharing bound.
    pub(super) base: i64,
    /// One entry per (request component, transformation chain) the rule list
    /// applies, which [`Self::total`] charges [`TEXT_TRANSFORMATION_WCU`] each.
    pub(super) transformations: ::std::collections::BTreeSet<u64>,
}

impl CapacityAccount {
    /// The WCUs the whole rule list consumes: every base cost, plus the
    /// transformation charge once per distinct (component, chain) pair.
    pub(super) fn total(&self) -> i64 {
        let charged = i64::try_from(self.transformations.len()).unwrap_or(i64::MAX);
        self.base
            .saturating_add(charged.saturating_mul(TEXT_TRANSFORMATION_WCU))
    }

    /// Charge one statement that inspects a request component: its `base` cost with
    /// the component's own modifiers applied, and the transformations it runs over
    /// that component.
    ///
    /// The component modifiers are the two the component itself carries:
    /// inspecting every query argument is a surcharge on top of the match
    /// ([`MULTI_VALUED_COMPONENT_WCU`]) and inspecting the body as JSON multiplies the
    /// match itself ([`JSON_BODY_BASE_MULTIPLIER`]) — so they COMPOSE with each
    /// other and with the per-transformation charge, which is flat.
    pub(super) fn add_inspection(
        &mut self,
        base: i64,
        field_to_match: &FieldToMatch,
        text_transformations: &[TextTransformation],
    ) {
        let component = component_key(field_to_match);
        self.base = self.base.saturating_add(
            base.saturating_mul(component_base_multiplier(field_to_match))
                .saturating_add(component_surcharge(field_to_match)),
        );
        self.note_transformations(&component, text_transformations);
    }

    /// Charge a statement that inspects no request component (a request attribute,
    /// a rate count, a referenced rule group).
    pub(super) fn add_base(&mut self, base: i64) {
        self.base = self.base.saturating_add(base);
    }

    /// Record the (component, chain) pairs `text_transformations` applies to
    /// `component`.
    ///
    /// "WAF performs all transformations on the specified content, starting from the
    /// lowest priority setting, and then uses the transformed component contents", so
    /// a transformation's work is the chain up to and including it: two statements
    /// over the same component with the same chain prefix share that work, and a
    /// chain that differs anywhere is work of its own. `NONE` transforms nothing, so
    /// it is neither charged nor part of the chain: a chain that only lists it
    /// alongside real transformations produces the same transformed content as one
    /// that leaves it out, so the two share the charge.
    ///
    /// Each pair is recorded as a 64-bit digest the chain is hashed into step by
    /// step, so the walk never holds a chain prefix of its own, and no more pairs are
    /// shared than [`MAX_SHARED_TRANSFORMATION_CHARGES`].
    pub(super) fn note_transformations(&mut self, component: &str, text_transformations: &[TextTransformation]) {
        use ::std::hash::{Hash, Hasher};

        let mut ordered: Vec<&TextTransformation> = text_transformations
            .iter()
            .filter(|transformation| transformation.r#type != TEXT_TRANSFORMATION_NONE)
            .collect();
        ordered.sort_by_key(|transformation| transformation.priority);
        let mut chain = ::std::collections::hash_map::DefaultHasher::new();
        component.hash(&mut chain);
        for transformation in ordered {
            transformation.r#type.hash(&mut chain);
            if self.transformations.len() >= MAX_SHARED_TRANSFORMATION_CHARGES {
                // Past the bound this accounting stops sharing and charges the
                // transformation on its own; see the constant.
                self.base = self.base.saturating_add(TEXT_TRANSFORMATION_WCU);
            } else {
                self.transformations.insert(chain.clone().finish());
            }
        }
    }

    /// Charge one statement and everything it nests. See [`web_acl_capacity`].
    ///
    /// `depth_budget` bounds the descent exactly as in
    /// [`statement_references_arn`]: a caller-supplied tree deeper than
    /// [`MAX_STATEMENT_NESTING_DEPTH`] is refused on ingress, and this budget is the
    /// second line of defence so no request can drive the recursion past it.
    pub(super) fn add_statement(
        &mut self,
        statement: &Statement,
        depth_budget: u32,
        rule_group_capacities: &HashMap<String, i64>,
    ) {
        if depth_budget == 0 {
            return;
        }
        if let Some(byte_match) = statement.byte_match_statement.as_ref() {
            self.add_inspection(
                byte_match_base(&byte_match.positional_constraint),
                &byte_match.field_to_match,
                &byte_match.text_transformations,
            );
        }
        if let Some(size_constraint) = statement.size_constraint_statement.as_ref() {
            self.add_inspection(
                ATTRIBUTE_MATCH_WCU,
                &size_constraint.field_to_match,
                &size_constraint.text_transformations,
            );
        }
        // The statements that decide from one already-extracted request attribute
        // and inspect no component of their own.
        for present in [
            statement.geo_match_statement.is_some(),
            statement.ip_set_reference_statement.is_some(),
            statement.label_match_statement.is_some(),
            statement.asn_match_statement.is_some(),
        ] {
            if present {
                self.add_base(ATTRIBUTE_MATCH_WCU);
            }
        }
        if let Some(sqli) = statement.sqli_match_statement.as_ref() {
            self.add_inspection(
                sqli_match_base(sqli.sensitivity_level.as_deref()),
                &sqli.field_to_match,
                &sqli.text_transformations,
            );
        }
        if let Some(xss) = statement.xss_match_statement.as_ref() {
            self.add_inspection(
                XSS_MATCH_WCU,
                &xss.field_to_match,
                &xss.text_transformations,
            );
        }
        if let Some(regex) = statement.regex_match_statement.as_ref() {
            self.add_inspection(
                REGEX_MATCH_WCU,
                &regex.field_to_match,
                &regex.text_transformations,
            );
        }
        if let Some(reference) = statement.regex_pattern_set_reference_statement.as_ref() {
            self.add_inspection(
                REGEX_PATTERN_SET_REFERENCE_WCU,
                &reference.field_to_match,
                &reference.text_transformations,
            );
        }
        // A rate-based statement costs `RATE_BASED_WCU` on its own, plus
        // `RATE_BASED_CUSTOM_KEY_WCU` for every custom aggregation key it extracts;
        // every nesting (logical operator or scope-down) costs what it nests, which
        // `nested_statements` enumerates.
        if let Some(rate) = statement.rate_based_statement.as_ref() {
            let keys = i64::try_from(rate.custom_keys.as_deref().unwrap_or_default().len())
                .unwrap_or(i64::MAX);
            self.add_base(
                RATE_BASED_WCU.saturating_add(keys.saturating_mul(RATE_BASED_CUSTOM_KEY_WCU)),
            );
        }
        // A rule group reference costs the referenced group's declared capacity, which
        // `rule_group_capacities` carries for every rule group of this scope. A
        // reference naming one this service does not hold never reaches here.
        if let Some(reference) = statement.rule_group_reference_statement.as_ref() {
            self.add_base(
                rule_group_capacities
                    .get(&reference.arn)
                    .copied()
                    .unwrap_or_default(),
            );
        }
        // A managed rule group reference costs the published capacity of the group it
        // names. A statement naming a group the catalogue does not list never reaches
        // here.
        if let Some(managed) = statement.managed_rule_group_statement.as_ref() {
            self.add_base(
                managed_rule_group(&managed.vendor_name, &managed.name)
                    .map(|group| group.capacity)
                    .unwrap_or_default(),
            );
        }
        for nested in nested_statements(statement) {
            self.add_statement(nested, depth_budget - 1, rule_group_capacities);
        }
    }
}

/// The base WCUs a `ByteMatchStatement` with this `PositionalConstraint` consumes.
///
/// See [`BYTE_MATCH_ANCHORED_WCU`] and [`BYTE_MATCH_SCANNING_WCU`]: the three
/// constraints that compare at a fixed position are the cheap ones, and anything
/// else searches the component.
pub(super) fn byte_match_base(positional_constraint: &str) -> i64 {
    match positional_constraint {
        "EXACTLY" | "STARTS_WITH" | "ENDS_WITH" => BYTE_MATCH_ANCHORED_WCU,
        _ => BYTE_MATCH_SCANNING_WCU,
    }
}

/// The base WCUs a `SqliMatchStatement` at this `SensitivityLevel` consumes. A
/// statement that names none is priced at the level the member documents as its
/// default; see [`SQLI_MATCH_LOW_WCU`].
pub(super) fn sqli_match_base(sensitivity_level: Option<&str>) -> i64 {
    if sensitivity_level == Some("HIGH") {
        SQLI_MATCH_HIGH_WCU
    } else {
        SQLI_MATCH_LOW_WCU
    }
}

/// What a statement's base cost is multiplied by for the component it inspects.
/// See [`JSON_BODY_BASE_MULTIPLIER`].
pub(super) fn component_base_multiplier(field_to_match: &FieldToMatch) -> i64 {
    if field_to_match.json_body.is_some() {
        JSON_BODY_BASE_MULTIPLIER
    } else {
        1
    }
}

/// What the component a statement inspects adds to its base cost: the surcharge
/// for every multi-valued component it names. See
/// [`MULTI_VALUED_COMPONENT_WCU`].
pub(super) fn component_surcharge(field_to_match: &FieldToMatch) -> i64 {
    let multi_valued = field_to_match.all_query_arguments.is_some()
        || field_to_match.headers.is_some()
        || field_to_match.cookies.is_some();
    if multi_valued {
        MULTI_VALUED_COMPONENT_WCU
    } else {
        0
    }
}

/// The request component a `FieldToMatch` names, as the key the transformation
/// charge is shared under: two statements over the SAME component share the work of
/// transforming it, and two over different components do not.
///
/// A single header or query argument names which one, because transforming one
/// header is not transforming another; the header name "isn't case sensitive"
/// (`FieldToMatch$SingleHeader`), so it is folded to one case. `FieldToMatch`
/// carries exactly one component ("include the single `FieldToMatch` type that you
/// want to inspect"), so the first one present is the component.
pub(super) fn component_key(field_to_match: &FieldToMatch) -> String {
    if let Some(header) = field_to_match.single_header.as_ref() {
        return format!("SingleHeader:{}", header.name.to_ascii_lowercase());
    }
    if let Some(argument) = field_to_match.single_query_argument.as_ref() {
        return format!("SingleQueryArgument:{}", argument.name.to_ascii_lowercase());
    }
    for (present, component) in [
        (field_to_match.all_query_arguments.is_some(), "AllQueryArguments"),
        (field_to_match.uri_path.is_some(), "UriPath"),
        (field_to_match.query_string.is_some(), "QueryString"),
        (field_to_match.body.is_some(), "Body"),
        (field_to_match.method.is_some(), "Method"),
        (field_to_match.json_body.is_some(), "JsonBody"),
        (field_to_match.headers.is_some(), "Headers"),
        (field_to_match.cookies.is_some(), "Cookies"),
        (field_to_match.header_order.is_some(), "HeaderOrder"),
        (field_to_match.ja3_fingerprint.is_some(), "JA3Fingerprint"),
        (field_to_match.ja4_fingerprint.is_some(), "JA4Fingerprint"),
        (field_to_match.uri_fragment.is_some(), "UriFragment"),
    ] {
        if present {
            return component.to_string();
        }
    }
    // A `FieldToMatch` naming no component at all: one key for all of them, so the
    // charge is still shared by whatever inspects it.
    "Unspecified".to_string()
}
