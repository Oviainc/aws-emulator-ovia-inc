// SPDX-License-Identifier: CC0-1.0

//! The stored classifier records — grok, XML, JSON and CSV — and the custom entity types a
//! create registers for detecting sensitive data.

use super::*;

/// A stored classifier: the custom classifier `CreateClassifier` registers for a crawler
/// to recognise a data format with.
///
/// The four classifier KINDS the model declares live in ONE record rather than four
/// namespaces, because every operation that addresses a classifier takes a `Name` alone
/// and the read answers with whichever kind that name holds. The members a client sets
/// live on [`ClassifierDefinition`]; the name and the three server-set members
/// (`Version`, `CreationTime`, `LastUpdated`) live here, because every kind declares the
/// same three and a read of any kind reports them the same way.
#[derive(Debug, Clone)]
pub struct ClassifierEntry {
    /// The classifier's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::classifiers`].
    pub name: String,
    /// "The time that this classifier was registered": the instant the create was served,
    /// so every later read of this record reports the same timestamp.
    pub creation_time: SystemTime,
    /// "The time that this classifier was last updated": the creation instant until a
    /// write lands, and the instant of the latest write afterwards.
    pub last_updated: SystemTime,
    /// "The version of this classifier": [`FIRST_CLASSIFIER_VERSION`] for a fresh one,
    /// advanced by one on every write.
    pub version: i64,
    /// Which kind of classifier this is, and the members the client set on it.
    pub definition: ClassifierDefinition,
}

/// What kind of classifier a stored record is, with the members that kind carries.
///
/// The kind is decided by which member of the create request was present — "This can be
/// a GrokClassifier, an XMLClassifier, a JsonClassifier, or a CsvClassifier, depending on
/// which field of the request is present" (`CreateClassifier`) — so the kind and its
/// members are ONE value: a stored classifier cannot hold the members of two kinds.
#[derive(Debug, Clone)]
pub enum ClassifierDefinition {
    /// "A classifier that uses `grok`."
    Grok {
        /// "An identifier of the data format that the classifier matches."
        classification: String,
        /// "The grok pattern applied to a data store by this classifier."
        grok_pattern: String,
        /// "Optional custom grok patterns defined by this classifier."
        custom_patterns: Option<String>,
    },
    /// "A classifier for XML content."
    Xml {
        /// "An identifier of the data format that the classifier matches."
        classification: String,
        /// "The XML tag designating the element that contains each record in an XML
        /// document being parsed."
        row_tag: Option<String>,
    },
    /// "A classifier for JSON content."
    Json {
        /// "A `JsonPath` string defining the JSON data for the classifier to classify."
        json_path: String,
    },
    /// "A classifier for comma-separated values (CSV)."
    Csv(CsvClassifierDefinition),
}

/// The members a CSV classifier carries beyond the name and the server-set three.
///
/// A struct of its own rather than nine fields on the enum variant, so one write path
/// applies them and one read path projects them.
#[derive(Debug, Clone, Default)]
pub struct CsvClassifierDefinition {
    /// "A custom symbol to denote what separates each column entry in the row."
    pub delimiter: Option<String>,
    /// "A custom symbol to denote what combines content into a single column value."
    pub quote_symbol: Option<String>,
    /// "Indicates whether the CSV file contains a header": a `CsvHeaderOption` value.
    pub contains_header: Option<String>,
    /// "A list of strings representing column names."
    pub header: Option<Vec<String>>,
    /// "Specifies not to trim values before identifying the type of column values."
    pub disable_value_trimming: Option<bool>,
    /// "Enables the processing of files that contain only one column."
    pub allow_single_column: Option<bool>,
    /// "Enables the custom datatype to be configured."
    pub custom_datatype_configured: Option<bool>,
    /// "A list of custom datatypes."
    pub custom_datatypes: Option<Vec<String>>,
    /// "Sets the SerDe for processing CSV in the classifier": a `CsvSerdeOption` value.
    pub serde: Option<String>,
}

/// The `CsvSerdeOption` a CSV classifier processes CSV with when its write named none.
///
/// Of the published values (`OpenCSVSerDe | LazySimpleSerDe | None`, from
/// `smithy_query shape=CsvSerdeOption`), the one that leaves the choice to detection: "you
/// can specify the `None` value when you want the crawler to do the detection"
/// (`CsvClassifier$Serde`). That is what a classifier naming no SerDe is set to, so it is a
/// VALUE the catalog holds for the member rather than an absence — which is why a read
/// reports it.
pub const CSV_SERDE_DETECTED: &str = "None";

/// Whether a CSV classifier's quote symbol is the character its column delimiter already
/// is, which the model forbids: "A custom symbol to denote what combines content into a
/// single column value. It must be different from the column delimiter"
/// (`CsvClassifier$QuoteSymbol`).
///
/// Takes the EFFECTIVE pair, so one rule serves a create (the members the request
/// carries) and an update (the members it carries merged over the stored ones). A pair
/// where either side is absent collides with nothing: the classifier names only one
/// separator.
pub fn csv_separators_collide(delimiter: Option<&str>, quote_symbol: Option<&str>) -> bool {
    matches!((delimiter, quote_symbol), (Some(delimiter), Some(quote_symbol)) if delimiter == quote_symbol)
}

impl ClassifierDefinition {
    /// The definition a `CreateClassifier` request carries, or `None` when the request
    /// named no classifier at all.
    ///
    /// Only ONE member is read, because the caller has already established that exactly
    /// one is present: the kind is "which field of the request is present", so a request
    /// naming several names no one kind.
    pub fn from_create_input(input: &CreateClassifierInput) -> Option<Self> {
        if let Some(grok) = input.grok_classifier.as_ref() {
            return Some(Self::Grok {
                classification: grok.classification.clone(),
                grok_pattern: grok.grok_pattern.clone(),
                custom_patterns: grok.custom_patterns.clone(),
            });
        }
        if let Some(xml) = input.xml_classifier.as_ref() {
            return Some(Self::Xml {
                classification: xml.classification.clone(),
                row_tag: xml.row_tag.clone(),
            });
        }
        if let Some(json) = input.json_classifier.as_ref() {
            return Some(Self::Json {
                json_path: json.json_path.clone(),
            });
        }
        let csv = input.csv_classifier.as_ref()?;
        Some(Self::Csv(CsvClassifierDefinition {
            delimiter: csv.delimiter.clone(),
            quote_symbol: csv.quote_symbol.clone(),
            contains_header: csv.contains_header.clone(),
            header: csv.header.clone(),
            disable_value_trimming: csv.disable_value_trimming,
            allow_single_column: csv.allow_single_column,
            custom_datatype_configured: csv.custom_datatype_configured,
            custom_datatypes: csv.custom_datatypes.clone(),
            serde: csv.serde.clone(),
        }))
    }

    /// The name of this kind, spelled as the model's member for it — the same spelling on
    /// `CreateClassifierRequest` and on `UpdateClassifierRequest`, so a refusal that names
    /// a kind names the member a request would carry it in.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Grok { .. } => "GrokClassifier",
            Self::Xml { .. } => "XMLClassifier",
            Self::Json { .. } => "JsonClassifier",
            Self::Csv(_) => "CsvClassifier",
        }
    }
}

/// How many of the four classifier kinds an `UpdateClassifier` request names, and which
/// kind and `Name` the one it names addresses.
///
/// The count and the target are read in one walk because an update needs both: the kind
/// is "which field is present" ("Modifies an existing classifier … depending on which
/// field is present"), and the name that field carries is the record it addresses.
pub fn update_classifier_target(
    input: &UpdateClassifierInput,
) -> (usize, Option<(&'static str, String)>) {
    let named = [
        input
            .grok_classifier
            .as_ref()
            .map(|request| ("GrokClassifier", request.name.clone())),
        input
            .xml_classifier
            .as_ref()
            .map(|request| ("XMLClassifier", request.name.clone())),
        input
            .json_classifier
            .as_ref()
            .map(|request| ("JsonClassifier", request.name.clone())),
        input
            .csv_classifier
            .as_ref()
            .map(|request| ("CsvClassifier", request.name.clone())),
    ];
    let count = named.iter().filter(|target| target.is_some()).count();
    (count, named.into_iter().flatten().next())
}

/// How many of the four classifier kinds a `CreateClassifier` request names.
///
/// The kind of the classifier to create is "which field of the request is present", so
/// this count is what decides whether the request names one kind, none, or an ambiguous
/// several.
pub fn named_classifier_kinds(input: &CreateClassifierInput) -> usize {
    [
        input.grok_classifier.is_some(),
        input.xml_classifier.is_some(),
        input.json_classifier.is_some(),
        input.csv_classifier.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count()
}

impl ClassifierEntry {
    /// Build a stored classifier from a `CreateClassifier` request served at `now`, or
    /// `None` when the request names no classifier kind.
    ///
    /// The definition comes from the request; the name is the one the named kind carries,
    /// and the three server-set members are the service's own: the create is the first
    /// version and both timestamps are the instant it was served.
    pub fn from_create_input(input: &CreateClassifierInput, now: SystemTime) -> Option<Self> {
        Some(Self {
            name: create_classifier_name(input)?,
            creation_time: now,
            last_updated: now,
            version: FIRST_CLASSIFIER_VERSION,
            definition: ClassifierDefinition::from_create_input(input)?,
        })
    }

    /// Apply an `UpdateClassifier` request to this stored classifier, member by member.
    ///
    /// Every member the request CARRIES replaces the stored one; every member it OMITS is
    /// preserved — the only reading an update whose definition members are all optional
    /// admits, and the same rule [`CrawlerEntry::apply_update`] follows. The server-set
    /// members move with the write, as a crawler's do: `Version` advances by one and
    /// `LastUpdated` becomes `now`. The kind, the `Name` and the `CreationTime` are not an
    /// update's to change, so the caller establishes that the request names THIS record's
    /// kind before applying it; a request for another kind leaves the definition alone.
    pub fn apply_update(&mut self, input: &UpdateClassifierInput, now: SystemTime) {
        match &mut self.definition {
            ClassifierDefinition::Grok {
                classification,
                grok_pattern,
                custom_patterns,
            } => {
                if let Some(request) = input.grok_classifier.as_ref() {
                    if let Some(value) = request.classification.as_ref() {
                        *classification = value.clone();
                    }
                    if let Some(value) = request.grok_pattern.as_ref() {
                        *grok_pattern = value.clone();
                    }
                    if let Some(value) = request.custom_patterns.as_ref() {
                        *custom_patterns = Some(value.clone());
                    }
                }
            }
            ClassifierDefinition::Xml {
                classification,
                row_tag,
            } => {
                if let Some(request) = input.xml_classifier.as_ref() {
                    if let Some(value) = request.classification.as_ref() {
                        *classification = value.clone();
                    }
                    if let Some(value) = request.row_tag.as_ref() {
                        *row_tag = Some(value.clone());
                    }
                }
            }
            ClassifierDefinition::Json { json_path } => {
                if let Some(request) = input.json_classifier.as_ref() {
                    if let Some(value) = request.json_path.as_ref() {
                        *json_path = value.clone();
                    }
                }
            }
            ClassifierDefinition::Csv(csv) => {
                if let Some(request) = input.csv_classifier.as_ref() {
                    if let Some(value) = request.delimiter.as_ref() {
                        csv.delimiter = Some(value.clone());
                    }
                    if let Some(value) = request.quote_symbol.as_ref() {
                        csv.quote_symbol = Some(value.clone());
                    }
                    if let Some(value) = request.contains_header.as_ref() {
                        csv.contains_header = Some(value.clone());
                    }
                    if let Some(value) = request.header.as_ref() {
                        csv.header = Some(value.clone());
                    }
                    if let Some(value) = request.disable_value_trimming {
                        csv.disable_value_trimming = Some(value);
                    }
                    if let Some(value) = request.allow_single_column {
                        csv.allow_single_column = Some(value);
                    }
                    if let Some(value) = request.custom_datatype_configured {
                        csv.custom_datatype_configured = Some(value);
                    }
                    if let Some(value) = request.custom_datatypes.as_ref() {
                        csv.custom_datatypes = Some(value.clone());
                    }
                    if let Some(value) = request.serde.as_ref() {
                        csv.serde = Some(value.clone());
                    }
                }
            }
        }
        self.version = self.version.saturating_add(1);
        self.last_updated = now;
    }

    /// The CSV members this classifier holds, or `None` when it is of another kind.
    ///
    /// What a CSV update's separator check reads: the rule is about the EFFECTIVE pair,
    /// which is the request's members merged over these.
    pub fn csv_definition(&self) -> Option<&CsvClassifierDefinition> {
        match &self.definition {
            ClassifierDefinition::Csv(csv) => Some(csv),
            _ => None,
        }
    }

    /// Rebuild the modelled `Classifier` shape from what is stored, so a listing reports
    /// the same classifier a single read does.
    ///
    /// Exactly one member of that shape is filled: the one for the kind this record is,
    /// which is how a client tells a grok classifier from a CSV one.
    pub fn to_classifier(&self) -> Classifier {
        let creation_time = Some(to_date_time(self.creation_time));
        let last_updated = Some(to_date_time(self.last_updated));
        match &self.definition {
            ClassifierDefinition::Grok {
                classification,
                grok_pattern,
                custom_patterns,
            } => Classifier {
                grok_classifier: Some(GrokClassifier {
                    name: self.name.clone(),
                    classification: classification.clone(),
                    creation_time,
                    last_updated,
                    version: self.version,
                    grok_pattern: grok_pattern.clone(),
                    custom_patterns: custom_patterns.clone(),
                }),
                ..Classifier::default()
            },
            ClassifierDefinition::Xml {
                classification,
                row_tag,
            } => Classifier {
                xml_classifier: Some(XMLClassifier {
                    name: self.name.clone(),
                    classification: classification.clone(),
                    creation_time,
                    last_updated,
                    version: self.version,
                    row_tag: row_tag.clone(),
                }),
                ..Classifier::default()
            },
            ClassifierDefinition::Json { json_path } => Classifier {
                json_classifier: Some(JsonClassifier {
                    name: self.name.clone(),
                    creation_time,
                    last_updated,
                    version: self.version,
                    json_path: json_path.clone(),
                }),
                ..Classifier::default()
            },
            ClassifierDefinition::Csv(csv) => Classifier {
                csv_classifier: Some(CsvClassifier {
                    name: self.name.clone(),
                    creation_time,
                    last_updated,
                    version: self.version,
                    delimiter: csv.delimiter.clone(),
                    quote_symbol: csv.quote_symbol.clone(),
                    contains_header: csv.contains_header.clone(),
                    header: csv.header.clone(),
                    disable_value_trimming: csv.disable_value_trimming,
                    allow_single_column: csv.allow_single_column,
                    custom_datatype_configured: csv.custom_datatype_configured,
                    custom_datatypes: csv.custom_datatypes.clone(),
                    // The SerDe is a choice the classifier always carries, so it is
                    // reported whether a write named it or left it at the detection
                    // option ([`CSV_SERDE_DETECTED`]): a client reading this member has
                    // to be able to tell which SerDe will be applied, and an absent
                    // member says nothing.
                    serde: Some(
                        csv.serde
                            .clone()
                            .unwrap_or_else(|| CSV_SERDE_DETECTED.to_string()),
                    ),
                }),
                ..Classifier::default()
            },
        }
    }
}

/// The name of the classifier a `CreateClassifier` request asks for: the `Name` the one
/// kind it names carries, or `None` when it names no kind.
///
/// Every kind's create request carries its own required `Name`, so the name is read off
/// the named kind rather than from a member of the request itself.
pub fn create_classifier_name(input: &CreateClassifierInput) -> Option<String> {
    if let Some(grok) = input.grok_classifier.as_ref() {
        return Some(grok.name.clone());
    }
    if let Some(xml) = input.xml_classifier.as_ref() {
        return Some(xml.name.clone());
    }
    if let Some(json) = input.json_classifier.as_ref() {
        return Some(json.name.clone());
    }
    input.csv_classifier.as_ref().map(|csv| csv.name.clone())
}

/// A stored custom entity type: the custom pattern `CreateCustomEntityType` registers
/// for detecting sensitive data.
///
/// Every member of the modelled `CustomEntityType` shape is held here, plus the create's
/// `Tags`, which that shape does not carry — so a later read reports the pattern the
/// create stored and nothing the server invented. There is no server-set member: the
/// model gives a custom entity type no id, status or timestamp.
#[derive(Debug, Clone)]
pub struct CustomEntityTypeEntry {
    /// The pattern's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::custom_entity_types`].
    pub name: String,
    /// "A regular expression string that is used for detecting sensitive data in a custom
    /// pattern", stored exactly as sent.
    pub regex_string: String,
    /// The context words the create supplied, if any. "If no context words are passed only
    /// a regular expression is checked", so a create that named none is stored with NO
    /// list rather than with an empty one: the two are different patterns, and the
    /// modelled list is 1..20 long, so an empty list is not even a value a create can send.
    pub context_words: Option<Vec<String>>,
    /// The create's `Tags`, which are not part of the `CustomEntityType` shape.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations: [`TaggedResourceKind`] carries no custom entity
    /// type variant, because the model states no ARN resource-type segment for this kind
    /// (nothing in it spells one), and the segment is what every tag lookup resolves an
    /// ARN against. So a tagging call naming a custom entity type still resolves to
    /// nothing, and these tags are stored rather than served.
    pub tags: HashMap<String, String>,
}

impl CustomEntityTypeEntry {
    /// Build a stored custom entity type from a `CreateCustomEntityType` request. Every
    /// member comes from the request: the client supplies the whole definition.
    pub fn from_create_input(input: &CreateCustomEntityTypeInput) -> Self {
        Self {
            name: input.name.clone(),
            regex_string: input.regex_string.clone(),
            context_words: input.context_words.clone(),
            tags: input.tags.clone().unwrap_or_default(),
        }
    }

    /// Rebuild the modelled `CustomEntityType` shape from what is stored, so a listing
    /// reports the same pattern a single read does. The stored `Tags` are not part of
    /// that shape, so they stay out of it.
    pub fn to_custom_entity_type(&self) -> CustomEntityType {
        CustomEntityType {
            name: self.name.clone(),
            regex_string: self.regex_string.clone(),
            context_words: self.context_words.clone(),
        }
    }
}
