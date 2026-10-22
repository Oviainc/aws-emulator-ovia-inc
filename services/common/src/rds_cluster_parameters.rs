// SPDX-License-Identifier: CC0-1.0

//! The cluster parameter catalog and parameter rules of the one AWS `rds` front door.
//!
//! Amazon RDS, Amazon DocumentDB and Amazon Neptune are three Smithy models of ONE AWS
//! front door (`AmazonRDSv19`, API version 2014-10-31, `awsQuery`,
//! `endpointPrefix`/`sigv4Name` `rds` on all three), so the cluster-parameter-group control
//! plane they expose is one control plane: the same `ParameterName`/`ParameterValue`
//! vocabulary, the same `ApplyMethod` enum, the same `Source` filter and the same engine
//! parameter catalog per family. Each of those crates keeps its own generated shapes and
//! its own state; what they share — the catalog, the value rule over it, and the rules the
//! model's documentation states for a parameter group's name, family and apply method —
//! lives here once, so two services cannot disagree about them.
//!
//! Nothing here names a modelled shape: it is pure data plus the rules over it, and each
//! service projects it onto its own `Parameter` / `DBClusterParameterGroup` shapes.

/// `Parameter$Source` as a response REPORTS it for a parameter whose value the caller set.
///
/// The request side and the response side use different names for the same three sources:
/// `DescribeDBClusterParametersMessage$Source` documents the filter vocabulary ("parameter
/// sources can be `engine`, `service`, or `customer`"), while a reported `Parameter$Source`
/// names the caller `user`, the engine's published default `engine-default` and the
/// service's own `system`. [`parameter_source_filter`] is the one place that mapping lives,
/// and both names of a source select it.
pub const PARAMETER_SOURCE_USER: &str = "user";
/// The reported `Parameter$Source` of a parameter carrying the engine's published default;
/// see [`PARAMETER_SOURCE_USER`].
pub const PARAMETER_SOURCE_ENGINE_DEFAULT: &str = "engine-default";
/// The reported `Parameter$Source` of a parameter the service itself sets; see
/// [`PARAMETER_SOURCE_USER`]. This emulator sets none, so nothing is reported under it.
pub const PARAMETER_SOURCE_SYSTEM: &str = "system";

/// `ApplyMethod`: the model's enum is `immediate | pending-reboot`
/// (`smithy_query(query="shape", shape="ApplyMethod")`).
pub const APPLY_METHOD_IMMEDIATE: &str = "immediate";
/// See [`APPLY_METHOD_IMMEDIATE`]. This is the method a change gets when the caller names
/// none: a parameter whose change has not been applied is the group's pending state, and
/// `Parameter$ApplyMethod` ("indicates when to apply parameter updates") carries it back.
pub const APPLY_METHOD_PENDING_REBOOT: &str = "pending-reboot";

/// `Parameter$ApplyType` for a parameter that takes effect only after the instances in the
/// cluster are rebooted.
pub const APPLY_TYPE_STATIC: &str = "static";
/// `Parameter$ApplyType` for a parameter that can take effect without a reboot.
pub const APPLY_TYPE_DYNAMIC: &str = "dynamic";

/// `ModifyDBClusterParameterGroup`'s own documentation: "A maximum of 20 parameters can be
/// modified in a single request." An over-bound request is refused, not truncated.
pub const MAX_PARAMETERS_PER_MODIFY: usize = 20;

/// The longest `DBClusterParameterGroupName` a service stores; see
/// [`validate_parameter_group_name`] for the model documentation that states it.
pub const MAX_PARAMETER_GROUP_NAME_LEN: usize = 255;

/// Which parameters a describe's `Source` asks for.
///
/// A `Source` is a FILTER, not a constraint: the member is documented as selecting
/// parameters "for a specific source", so a value that names no source this service knows
/// selects nothing rather than being refused — the same answer the real service gives, and
/// the reason a `Source` can never turn a missing group's 404 into a 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterSourceFilter {
    /// The group's own parameters: the ones a caller set.
    Customer,
    /// The parameters the engine family publishes, with their default values.
    Engine,
    /// The parameters the service itself sets; this emulator sets none.
    Service,
    /// A value naming no source this service knows: it selects nothing.
    Unknown,
}

/// The source a describe's `Source` names, by either of that source's two names, defaulting
/// to the group's own parameters when the caller names none; see [`PARAMETER_SOURCE_USER`]
/// for where the two vocabularies come from.
pub fn parameter_source_filter(source: Option<&str>) -> ParameterSourceFilter {
    match source {
        None => ParameterSourceFilter::Customer,
        Some(value) if value == "customer" || value == PARAMETER_SOURCE_USER => {
            ParameterSourceFilter::Customer
        }
        Some(value) if value == "engine" || value == PARAMETER_SOURCE_ENGINE_DEFAULT => {
            ParameterSourceFilter::Engine
        }
        Some(value) if value == "service" || value == PARAMETER_SOURCE_SYSTEM => {
            ParameterSourceFilter::Service
        }
        Some(_) => ParameterSourceFilter::Unknown,
    }
}

/// One parameter an engine family publishes: what `DescribeDBClusterParameters` reports
/// about it and what `ModifyDBClusterParameterGroup` admits as its value.
#[derive(Debug, Clone, Copy)]
pub struct ClusterParameterSpec {
    /// `Parameter$ParameterName`.
    pub name: &'static str,
    /// `Parameter$ApplyType`; see [`APPLY_TYPE_STATIC`].
    pub apply_type: &'static str,
    /// `Parameter$DataType`: `string`, `integer` or `float`, which is also how
    /// [`Self::admits`] reads [`Self::allowed_values`].
    pub data_type: &'static str,
    /// `Parameter$AllowedValues`: either a comma-separated enumeration of the admitted
    /// values, or a `<low>-<high>` range for a numeric parameter.
    pub allowed_values: &'static str,
    /// The value the engine runs with until a caller sets one.
    pub default_value: &'static str,
    /// `Parameter$IsModifiable`.
    pub is_modifiable: bool,
    /// `Parameter$MinimumEngineVersion`.
    pub minimum_engine_version: &'static str,
    /// `Parameter$Description`.
    pub description: &'static str,
}

impl ClusterParameterSpec {
    /// Whether this parameter admits `value`, reading [`Self::allowed_values`] the way
    /// [`Self::data_type`] says: a comma-separated enumeration is matched exactly, and a
    /// `<low>-<high>` range is parsed as the numeric type and bounds-checked inclusively.
    pub fn admits(&self, value: &str) -> bool {
        if self.allowed_values.contains(',') {
            return self
                .allowed_values
                .split(',')
                .map(str::trim)
                .any(|allowed| allowed == value);
        }
        match self.allowed_values.split_once('-') {
            Some((low, high)) if self.data_type == "integer" => {
                match (
                    value.parse::<i64>(),
                    low.trim().parse::<i64>(),
                    high.trim().parse::<i64>(),
                ) {
                    (Ok(value), Ok(low), Ok(high)) => (low..=high).contains(&value),
                    _ => false,
                }
            }
            Some((low, high)) if self.data_type == "float" => {
                match (
                    value.parse::<f64>(),
                    low.trim().parse::<f64>(),
                    high.trim().parse::<f64>(),
                ) {
                    (Ok(value), Ok(low), Ok(high)) => value >= low && value <= high,
                    _ => false,
                }
            }
            _ => self.allowed_values == value,
        }
    }
}

/// The prefix of every Amazon DocumentDB cluster parameter group family (`docdb3.6`,
/// `docdb4.0`, `docdb5.0`), which is how [`published_parameters`] decides whether this
/// catalog covers the family a group was created with.
pub const DOCDB_PARAMETER_GROUP_FAMILY_PREFIX: &str = "docdb";

/// The cluster parameter group families Amazon DocumentDB publishes, as the Amazon
/// DocumentDB Developer Guide lists them (one per engine major version: 3.6, 4.0 and 5.0).
/// A `docdb`-prefixed family outside this set names no DocumentDB engine version.
pub const DOCDB_PARAMETER_GROUP_FAMILIES: [&str; 3] = ["docdb3.6", "docdb4.0", "docdb5.0"];

/// The cluster parameters the Amazon DocumentDB families publish.
///
/// Source: the Amazon DocumentDB Developer Guide's "Amazon DocumentDB cluster parameters"
/// page, which lists each parameter with its allowed values, default, data type, apply type
/// and the engine version it appeared in. `tls`'s allowed values are also in the model's own
/// documentation — `smithy_query(query="doc", shape="Parameter", member="ParameterValue")`
/// reads "Valid values are: enabled, disabled, fips-140-3, tls1.2+, tls1.3+".
///
/// `allowed_values` is written the way the API reports it: a comma-separated enumeration
/// with no spaces, or a `<low>-<high>` range for a numeric parameter.
///
/// `description` is the text the API itself reports for the parameter, as the guide's
/// `describe-db-cluster-parameters` / `describe-engine-default-cluster-parameters` output
/// prints it ("Enables auditing on cluster.", "Config to enable/disable TLS", …) — a
/// paraphrase would report a `Parameter$Description` the service never reports.
///
/// The catalog is not narrowed per family or Region: the model documents the value set
/// without that distinction, so a value it lists is admitted for every `docdb` family
/// rather than refused on a guess about which engine version carries it.
pub const DOCDB_CLUSTER_PARAMETERS: &[ClusterParameterSpec] = &[
    ClusterParameterSpec {
        name: "audit_logs",
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "string",
        allowed_values: "enabled,disabled,all,ddl,dml_read,dml_write",
        default_value: "disabled",
        is_modifiable: true,
        minimum_engine_version: "4.0.0",
        description: "Enables auditing on cluster.",
    },
    ClusterParameterSpec {
        name: "change_stream_log_retention_duration",
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        allowed_values: "3600-86400",
        default_value: "10800",
        is_modifiable: true,
        minimum_engine_version: "4.0.0",
        description: "Duration of time in seconds change stream log is retained and can be \
                      consumed.",
    },
    ClusterParameterSpec {
        name: "profiler",
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "string",
        allowed_values: "enabled,disabled",
        default_value: "disabled",
        is_modifiable: true,
        minimum_engine_version: "4.0.0",
        description: "Enables database profiler.",
    },
    ClusterParameterSpec {
        name: "profiler_sampling_rate",
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "float",
        allowed_values: "0.0-1.0",
        default_value: "1.0",
        is_modifiable: true,
        minimum_engine_version: "4.0.0",
        description: "Fraction of slow operations that should be profiled or logged.",
    },
    ClusterParameterSpec {
        name: "profiler_threshold_ms",
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "integer",
        allowed_values: "50-2147483646",
        default_value: "100",
        is_modifiable: true,
        minimum_engine_version: "4.0.0",
        description: "Operations taking longer than profiler_threshold_ms are logged or \
                      profiled.",
    },
    ClusterParameterSpec {
        name: "tls",
        apply_type: APPLY_TYPE_STATIC,
        data_type: "string",
        allowed_values: "enabled,disabled,fips-140-3,tls1.2+,tls1.3+",
        default_value: "enabled",
        is_modifiable: true,
        minimum_engine_version: "3.6.0",
        description: "Config to enable/disable TLS",
    },
    ClusterParameterSpec {
        name: "ttl_monitor",
        apply_type: APPLY_TYPE_DYNAMIC,
        data_type: "string",
        allowed_values: "enabled,disabled",
        default_value: "enabled",
        is_modifiable: true,
        minimum_engine_version: "3.6.0",
        description: "Enables TTL Monitoring",
    },
];

/// The parameters this emulator publishes for `family`, or `None` when it publishes none.
///
/// A group can be created with any family string the create admits — the one `rds` front
/// door serves the DocumentDB, Amazon RDS and Neptune engine families — and this catalog
/// covers the DocumentDB families ([`DOCDB_PARAMETER_GROUP_FAMILIES`]) only, so it answers
/// for exactly the families [`validate_parameter_group_family`] recognises as DocumentDB's.
/// A family it publishes nothing for gets no value validation and no reported parameter
/// metadata: refusing a value, or reporting an allowed-value set, for a family whose
/// parameters are not published here would invent a constraint.
pub fn published_parameters(family: &str) -> Option<&'static [ClusterParameterSpec]> {
    let family = family.to_ascii_lowercase();
    DOCDB_PARAMETER_GROUP_FAMILIES
        .contains(&family.as_str())
        .then_some(DOCDB_CLUSTER_PARAMETERS)
}

/// The spec `family` publishes for the parameter named `name`, if any. Parameter names are
/// matched exactly: `Filter$Name`'s "filter names are case sensitive" is the API's habit
/// throughout, and the engine's parameter names are lowercase.
pub fn published_parameter(family: &str, name: &str) -> Option<&'static ClusterParameterSpec> {
    published_parameters(family)?
        .iter()
        .find(|spec| spec.name == name)
}

/// Whether a change to `name` with apply method `apply_method` is still waiting on a reboot
/// of the cluster's instances.
///
/// `ModifyDBClusterParameterGroup`'s documentation is the rule: "Changes to dynamic
/// parameters are applied immediately. Changes to static parameters require a reboot or
/// maintenance window before the change can take effect." So a change the caller asked to
/// apply [`APPLY_METHOD_PENDING_REBOOT`], and any change to a parameter whose published
/// apply type is [`APPLY_TYPE_STATIC`], is pending.
pub fn change_is_pending(family: &str, name: &str, apply_method: &str) -> bool {
    apply_method == APPLY_METHOD_PENDING_REBOOT
        || published_parameter(family, name).is_some_and(|spec| spec.apply_type == APPLY_TYPE_STATIC)
}

/// The method a change gets when the caller asks for none: the one the parameter's own apply
/// type implies.
///
/// Same documented rule as [`change_is_pending`] — "Changes to dynamic parameters are applied
/// immediately. Changes to static parameters require a reboot or maintenance window before the
/// change can take effect." So a change to a parameter whose published apply type is
/// [`APPLY_TYPE_DYNAMIC`] is [`APPLY_METHOD_IMMEDIATE`], and one to a static parameter — or to
/// a parameter whose family publishes no apply type, where assuming the change needs no reboot
/// would be a guess — waits for the reboot.
pub fn default_apply_method(family: &str, name: &str) -> &'static str {
    match published_parameter(family, name) {
        Some(spec) if spec.apply_type == APPLY_TYPE_DYNAMIC => APPLY_METHOD_IMMEDIATE,
        _ => APPLY_METHOD_PENDING_REBOOT,
    }
}

/// Refuse a parameter a group cannot carry: one its family does not publish, one the engine
/// does not let a caller modify, or a value outside the ones the parameter admits.
///
/// `Err` carries the message to answer with, built from the request — the parameter the
/// caller named and the values that parameter admits — so the caller can fix the request
/// from the text alone. A family this emulator publishes no catalog for admits any
/// parameter and any value; see [`published_parameters`].
pub fn validate_parameter_setting(family: &str, name: &str, value: &str) -> Result<(), String> {
    let Some(published) = published_parameters(family) else {
        return Ok(());
    };
    let Some(spec) = published.iter().find(|spec| spec.name == name) else {
        let names: Vec<&str> = published.iter().map(|spec| spec.name).collect();
        return Err(format!(
            "Invalid parameter name '{name}': the {family} cluster parameter group family \
             publishes no parameter with that name; its parameters are {}",
            names.join(", ")
        ));
    };
    if !spec.is_modifiable {
        return Err(format!(
            "Invalid value '{value}' for parameter '{name}': the parameter is not modifiable"
        ));
    }
    if !spec.admits(value) {
        return Err(format!(
            "Invalid value '{value}' for parameter '{name}': allowed values are {}",
            spec.allowed_values
        ));
    }
    Ok(())
}

/// Refuse the name of a parameter a reset names that its family does not publish; see
/// [`validate_parameter_setting`], whose value half does not apply to a reset.
pub fn validate_parameter_name(family: &str, name: &str) -> Result<(), String> {
    if published_parameters(family).is_none() || published_parameter(family, name).is_some() {
        return Ok(());
    }
    Err(format!(
        "Invalid parameter name '{name}': the {family} cluster parameter group family publishes \
         no parameter with that name"
    ))
}

/// Refuse a `DBClusterParameterGroupName` outside the rule the model states for it.
///
/// `DBClusterParameterGroupNameMessage$DBClusterParameterGroupName` — the name a modify or
/// reset answers with, i.e. the name of a cluster parameter group — states the rule:
/// "Constraints: Must be from 1 to 255 letters or numbers. The first character must be a
/// letter. Cannot end with a hyphen or contain two consecutive hyphens." So a name carrying
/// a character outside that set (a `.`, which is what a `default.<family>` group's name
/// carries), one that does not start with a letter, or one past the bound is refused
/// instead of stored.
pub fn validate_parameter_group_name(member: &str, value: &str) -> Result<(), String> {
    let well_formed = !value.is_empty()
        && value.chars().count() <= MAX_PARAMETER_GROUP_NAME_LEN
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && value.starts_with(|c: char| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--");
    if well_formed {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for {member}: it must contain from 1 to \
         {MAX_PARAMETER_GROUP_NAME_LEN} letters, numbers, or hyphens, the first character must \
         be a letter, and it cannot end with a hyphen or contain two consecutive hyphens"
    ))
}

/// Refuse a `DBParameterGroupFamily` that names a DocumentDB engine family that does not
/// exist.
///
/// The families Amazon DocumentDB publishes are [`DOCDB_PARAMETER_GROUP_FAMILIES`]. A family
/// that does not name the DocumentDB engine at all belongs to one of the other engines the
/// shared `rds` front door serves, and is accepted: this catalog publishes nothing for it
/// (see [`published_parameters`]) and refusing it would invent a constraint.
pub fn validate_parameter_group_family(value: &str) -> Result<(), String> {
    let lowered = value.to_ascii_lowercase();
    if !lowered.starts_with(DOCDB_PARAMETER_GROUP_FAMILY_PREFIX)
        || DOCDB_PARAMETER_GROUP_FAMILIES.contains(&lowered.as_str())
    {
        return Ok(());
    }
    Err(format!(
        "Invalid value '{value}' for DBParameterGroupFamily: the Amazon DocumentDB families are \
         {}",
        DOCDB_PARAMETER_GROUP_FAMILIES.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    //! The rules over the catalog, asserted over the catalog ITSELF rather than over one
    //! parameter: every entry, each enumerated value, each published range bound and one
    //! step past each bound.

    use super::*;

    /// Every published parameter admits exactly what its own `AllowedValues` names.
    #[test]
    fn a_parameter_admits_exactly_its_published_values() {
        for spec in DOCDB_CLUSTER_PARAMETERS {
            if spec.allowed_values.contains(',') {
                for value in spec.allowed_values.split(',') {
                    assert!(
                        spec.admits(value.trim()),
                        "case {}: {value:?} is published for it, so it must be admitted",
                        spec.name
                    );
                }
                for value in ["", "not-a-published-value", "ENABLED"] {
                    assert!(
                        !spec.admits(value),
                        "case {}: {value:?} is not published for it, so it must be refused",
                        spec.name
                    );
                }
                continue;
            }
            let (low, high) = spec
                .allowed_values
                .split_once('-')
                .expect("a non-enumerated AllowedValues is a <low>-<high> range");
            // Each published bound is admitted and one step past each is refused, read as
            // the parameter's own data type.
            match spec.data_type {
                "integer" => {
                    let low: i64 = low.parse().expect("an integer bound");
                    let high: i64 = high.parse().expect("an integer bound");
                    for value in [low, high] {
                        assert!(
                            spec.admits(&value.to_string()),
                            "case {}: the published bound {value} must be admitted",
                            spec.name
                        );
                    }
                    for value in [low - 1, high + 1] {
                        assert!(
                            !spec.admits(&value.to_string()),
                            "case {}: {value} is outside the published range, so it must be \
                             refused",
                            spec.name
                        );
                    }
                }
                "float" => {
                    let low: f64 = low.parse().expect("a float bound");
                    let high: f64 = high.parse().expect("a float bound");
                    for value in [low, high] {
                        assert!(
                            spec.admits(&value.to_string()),
                            "case {}: the published bound {value} must be admitted",
                            spec.name
                        );
                    }
                    for value in [low - 1.0, high + 1.0] {
                        assert!(
                            !spec.admits(&value.to_string()),
                            "case {}: {value} is outside the published range, so it must be \
                             refused",
                            spec.name
                        );
                    }
                }
                other => panic!("unhandled DataType {other:?} on {}", spec.name),
            }
            assert!(
                !spec.admits("not-a-number"),
                "case {}: a numeric parameter must refuse a value that is not a number",
                spec.name
            );
        }
    }

    /// A family the catalog covers validates values; one it does not covers nothing, so it
    /// admits any parameter and any value. The two predicates agree about which families
    /// exist: a `docdb` family the engine does not have is refused at create time AND
    /// publishes no catalog.
    #[test]
    fn only_a_published_family_validates_a_value() {
        // (case, family, publishes a catalog)
        let rows: [(&str, &str, bool); 6] = [
            ("the 3.6 family", "docdb3.6", true),
            ("the 4.0 family", "docdb4.0", true),
            ("the 5.0 family", "docdb5.0", true),
            ("a family the engine does not have", "docdb9.9", false),
            ("another engine's family", "aurora-mysql8.0", false),
            ("a neptune family", "neptune1.3", false),
        ];

        for (case, family, publishes) in rows {
            assert_eq!(
                published_parameters(family).is_some(),
                publishes,
                "case {case:?}: expected published_parameters({family:?}).is_some() == {publishes}"
            );
            let unknown = validate_parameter_setting(family, "no_such_parameter", "1");
            assert_eq!(
                unknown.is_err(),
                publishes,
                "case {case:?}: a parameter outside a published catalog must be refused, and a \
                 family with no catalog must take it, got {unknown:?}"
            );
            let refused_value = validate_parameter_setting(family, "tls", "not-a-tls-value");
            assert_eq!(
                refused_value.is_err(),
                publishes,
                "case {case:?}: a value outside the published set must be refused only where the \
                 family is published, got {refused_value:?}"
            );
        }

        // Only a `docdb`-prefixed family that names no DocumentDB engine version is refused
        // at create time; another engine's family is served by the same front door.
        for (case, family, admitted) in [
            ("a published family", "docdb5.0", true),
            ("a docdb family that does not exist", "docdb9.9", false),
            ("another engine's family", "aurora-postgresql15", true),
        ] {
            assert_eq!(
                validate_parameter_group_family(family).is_ok(),
                admitted,
                "case {case:?}: expected validate_parameter_group_family({family:?}).is_ok() == \
                 {admitted}"
            );
        }
    }

    /// A change is pending when the caller asked for the reboot, or when the parameter can
    /// only take effect on one.
    #[test]
    fn a_static_or_deferred_change_is_pending() {
        // (case, parameter, apply method, pending)
        let rows: [(&str, &str, &str, bool); 5] = [
            ("a static parameter, immediate", "tls", APPLY_METHOD_IMMEDIATE, true),
            (
                "a static parameter, pending-reboot",
                "tls",
                APPLY_METHOD_PENDING_REBOOT,
                true,
            ),
            ("a dynamic parameter, immediate", "profiler", APPLY_METHOD_IMMEDIATE, false),
            (
                "a dynamic parameter, pending-reboot",
                "profiler",
                APPLY_METHOD_PENDING_REBOOT,
                true,
            ),
            (
                // A family with no catalog publishes no apply type, so only the method the
                // caller asked for decides.
                "an unpublished family, immediate",
                "max_connections",
                APPLY_METHOD_IMMEDIATE,
                false,
            ),
        ];

        for (case, name, apply_method, pending) in rows {
            let family = if name == "max_connections" {
                "aurora-mysql8.0"
            } else {
                "docdb5.0"
            };
            assert_eq!(
                change_is_pending(family, name, apply_method),
                pending,
                "case {case:?}: expected change_is_pending({family:?}, {name:?}, \
                 {apply_method:?}) == {pending}"
            );
        }
    }

    /// Both names of each source select it; a value naming none selects nothing; an absent
    /// one is the group's own parameters.
    #[test]
    fn both_names_of_a_source_select_it() {
        // (case, Source, selected)
        let rows: [(&str, Option<&str>, ParameterSourceFilter); 8] = [
            ("absent", None, ParameterSourceFilter::Customer),
            ("the request vocabulary", Some("customer"), ParameterSourceFilter::Customer),
            (
                "the reported vocabulary",
                Some(PARAMETER_SOURCE_USER),
                ParameterSourceFilter::Customer,
            ),
            ("the engine", Some("engine"), ParameterSourceFilter::Engine),
            (
                "the engine, as reported",
                Some(PARAMETER_SOURCE_ENGINE_DEFAULT),
                ParameterSourceFilter::Engine,
            ),
            ("the service", Some("service"), ParameterSourceFilter::Service),
            (
                "the service, as reported",
                Some(PARAMETER_SOURCE_SYSTEM),
                ParameterSourceFilter::Service,
            ),
            ("no source this API knows", Some("not-a-source"), ParameterSourceFilter::Unknown),
        ];

        for (case, source, expected) in rows {
            let selected = parameter_source_filter(source);
            assert_eq!(
                selected, expected,
                "case {case:?} ({source:?}): expected {expected:?}, got {selected:?}"
            );
        }
    }

    /// The name rule, over every clause the model states and both sides of its bound.
    #[test]
    fn a_group_name_follows_the_documented_rule() {
        let longest = format!("a{}", "b".repeat(MAX_PARAMETER_GROUP_NAME_LEN - 1));
        let too_long = format!("a{}", "b".repeat(MAX_PARAMETER_GROUP_NAME_LEN));
        // (case, name, admitted)
        let rows: [(&str, &str, bool); 9] = [
            ("letters, digits and hyphens", "probe-cpg-1", true),
            ("the longest admitted name", &longest, true),
            ("one character past the bound", &too_long, false),
            ("an empty name", "", false),
            ("starting with a digit", "1-probe", false),
            ("starting with a hyphen", "-probe", false),
            ("ending with a hyphen", "probe-", false),
            ("two consecutive hyphens", "pro--be", false),
            ("a default group's name, which carries a dot", "default.docdb5.0", false),
        ];

        for (case, name, admitted) in rows {
            let checked = validate_parameter_group_name("DBClusterParameterGroupName", name);
            assert_eq!(
                checked.is_ok(),
                admitted,
                "case {case:?}: expected admitted == {admitted}, got {checked:?}"
            );
        }
    }
}
