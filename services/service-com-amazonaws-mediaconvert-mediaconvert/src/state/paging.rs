// SPDX-License-Identifier: CC0-1.0

//! How every paged reader of this service serves one page: the cursor it resumes from,
//! the ordering it walks the records in, and the page each listing door projects.
//!
//! Moved out of `super` unchanged, so one file holds the paging concern every `List*`
//! door shares and the state module keeps the records themselves.

use super::*;

/// One page of records and the cursor that resumes after it.
#[derive(Debug)]
pub struct Page<'a, R> {
    pub records: Vec<&'a R>,
    /// The [`encode_cursor`] token of the last record of the page, and only when a further
    /// page exists.
    pub next_token: Option<String>,
}

/// The page of records EVERY paged reader in this service reports.
///
/// ONE place that owns the ordering, the cursor and the page size, whatever kind of record
/// is being listed, so no paged reader can drift from another: a change to the anchor
/// comparison, the page-size fallback or the token condition lands in all of them at once.
/// A listing runs UP the key it is ordered on — alphabetically from the start of the
/// alphabet, chronologically from the oldest — unless the caller asks for
/// [`ORDER_DESCENDING`]; `Order`'s doc says only "Default varies by resource", so each
/// listing passes the `default_order` its own member documents, for the request that
/// states none.
///
/// `records` is whatever set the handler selected (a `Type`, a category, a filter):
/// choosing the set is the handler's business, ordering and paging it is this function's.
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to; a token that
/// decodes to nothing is refused by the handler that read it, with its modelled
/// `BadRequestException`, so there is no failure left to answer here.
pub fn page_of_records<'a, R: PagedRecord>(
    records: impl Iterator<Item = &'a R>,
    sort_key: ListBySortKey,
    order: Option<&str>,
    default_order: &str,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> Page<'a, R> {
    let ascending = order.unwrap_or(default_order) != ORDER_DESCENDING;
    let mut matching: Vec<&R> = records.collect();
    matching.sort_by(|left, right| sort_key.compare(left.ordering_key(), right.ordering_key()));
    if !ascending {
        matching.reverse();
    }

    let start = match resume_from {
        None => 0,
        Some((nanos, id)) => {
            let anchor = (*nanos, id.as_str());
            // Resume at the first record strictly past the anchor key in this direction.
            // The anchor itself need not still be in the result set, so a record deleted or
            // changed between two pages is skipped rather than failing the token.
            matching
                .iter()
                .position(|record| {
                    let side = sort_key.compare(record.ordering_key(), anchor);
                    if ascending {
                        side == ::std::cmp::Ordering::Greater
                    } else {
                        side == ::std::cmp::Ordering::Less
                    }
                })
                .unwrap_or(matching.len())
        }
    };

    // Every `maxResults` member carries the model's own 1..20 range, so the framework has
    // already refused anything outside it; the fallback only covers the absent case.
    let page_size = usize::try_from(max_results.unwrap_or(DEFAULT_PAGE_SIZE))
        .unwrap_or(DEFAULT_PAGE_SIZE as usize);
    let page = &matching[start.min(matching.len())..];
    let page = &page[..page_size.min(page.len())];
    // A token only when there is a further page to fetch.
    let next_token = (start + page.len() < matching.len())
        .then(|| page.last().map(|record| record.cursor()))
        .flatten();
    Page {
        records: page.to_vec(),
        next_token,
    }
}

/// The jobs the filters a paged jobs reader carries keep, out of every job the account
/// holds.
///
/// ONE place that owns the SET a filtering jobs reader reports, as [`records_listed_by`]
/// owns the set a `listBy` listing reports and [`page_of_records`] the ordering, the cursor
/// and the page size, so no two jobs readers can drift:
///
/// * every filter the request carries NARROWS the set, so a job is kept only when it
///   satisfies all of them, and a filter the request leaves out narrows nothing;
/// * `queue` keeps the jobs sent to exactly that queue ([`JobRecord::matches_queue`]),
///   compared by queue name — a member that also offers the ARN form is reduced to the name
///   by the reader that read it ([`queue_name_from_reference`]), because only a member whose
///   own documentation offers that form may be reduced;
/// * `status` keeps the jobs in exactly that status ([`JobRecord::matches_status`]);
/// * `input_file` keeps the jobs one of whose inputs CONTAINS it
///   ([`JobRecord::matches_input_file`]), since such a member is "your input file URL or
///   your partial input file name".
///
/// `ListJobs` and `SearchJobs` both narrow the job set here, each passing the filters its
/// own members offer; the jobs query takes the very same three tests one at a time
/// ([`JobRecord::matches_filter`]), because its filter list combines them with OR rather
/// than AND.
pub fn jobs_matching<'a>(
    jobs: impl Iterator<Item = &'a JobRecord> + 'a,
    queue: Option<&'a str>,
    status: Option<&'a str>,
    input_file: Option<&'a str>,
) -> impl Iterator<Item = &'a JobRecord> + 'a {
    jobs.filter(move |job| {
        queue.is_none_or(|name| job.matches_queue(name))
            && status.is_none_or(|want| job.matches_status(want))
            && input_file.is_none_or(|file| job.matches_input_file(file))
    })
}

/// The page of `jobs` a paged jobs reader reports, through [`page_of_records`].
///
/// Every paged reader of the job set goes through this one function — `ListJobs`, `SearchJobs`
/// and the page a jobs query matches — so they all state the one default this listing
/// documents. "Your most recently created jobs": ordered by creation time, newest first
/// unless the caller asked for [`ORDER_ASCENDING`], with the id breaking ties.
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to.
pub fn page_of_jobs<'a>(
    jobs: impl Iterator<Item = &'a JobRecord>,
    order: Option<&str>,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> Page<'a, JobRecord> {
    page_of_records(
        jobs,
        ListBySortKey::CreationDate,
        order,
        ORDER_DESCENDING,
        resume_from,
        max_results,
    )
}

/// Which key a `listBy`-ordered listing is ordered on, as `listBy` chooses it. Every
/// `*ListBy` enum in this service names the same two keys — `QueueListBy`, for one: "you
/// can choose to list them alphabetically by NAME or chronologically by CREATION_DATE" —
/// so ONE type carries both, and each listing supplies the default its own member
/// documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListBySortKey {
    /// Alphabetically by `Name`. Names are unique within the account, so this key alone
    /// is a total order.
    Name,
    /// Chronologically by `CreatedAt`, with the name breaking ties.
    CreationDate,
}

impl ListBySortKey {
    /// The key the `listBy` a request carried names, or `default` when it carried none —
    /// the value that member's own doc states ("If you don't specify, the service will
    /// list them by ..."). A `listBy` value that names neither key, such as
    /// [`LIST_BY_SYSTEM`], selects a SET rather than a key, so it orders on `default`
    /// too.
    pub fn from_list_by(list_by: Option<&str>, default: Self) -> Self {
        match list_by {
            Some(LIST_BY_NAME) => Self::Name,
            Some(LIST_BY_CREATION_DATE) => Self::CreationDate,
            _ => default,
        }
    }

    /// Where one record's `ordering_key` falls against another's on this key.
    pub fn compare(self, left: (u128, &str), right: (u128, &str)) -> ::std::cmp::Ordering {
        match self {
            Self::Name => left.1.cmp(right.1),
            Self::CreationDate => left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)),
        }
    }
}

/// A record whose listing selects it with a `listBy` and a `category`.
///
/// Implemented by the two kinds of record whose listing takes both members — a job
/// template and a preset — so [`records_listed_by`] owns the one selection rule they
/// share. `is_system` comes from [`NamedRecord`], which already states which records are
/// built in for the create and delete paths, so the set half of a listing reads the same
/// property those do.
pub trait ListedRecord: NamedRecord {
    /// The `Category` the record is stored under, or `None` for one stored under no
    /// category at all.
    fn record_category(&self) -> Option<&str>;
}

impl ListedRecord for JobTemplateRecord {
    fn record_category(&self) -> Option<&str> {
        self.category.as_deref()
    }
}

impl ListedRecord for PresetRecord {
    fn record_category(&self) -> Option<&str> {
        self.category.as_deref()
    }
}

/// The records a request's `listBy` and `category` select, out of every record of that
/// kind the account holds.
///
/// ONE place that owns the SET a `listBy` listing reports, as [`page_of_records`] owns its
/// ordering, cursor and page size, so no two such listings can drift:
///
/// * `listBy` chooses WHICH set: its [`LIST_BY_SYSTEM`] value selects the built-in records,
///   the ones a caller can neither modify nor delete, and every other value — including
///   none at all, which names only an ordering key — the account's own records.
/// * `category` keeps only the records stored under exactly that category, so a record
///   stored under none matches no category a caller can name ("specify a ... category to
///   limit responses to only [the records] from that category"); a request naming no
///   category narrows nothing.
///
/// A `category` carrying no characters never reaches here: both listings refuse such a
/// value at the door ([`blank_member_problem`]), because [`optional_free_text`] stores a
/// category only when it carries characters, so an equality test against one could match
/// no record and would report a narrowing to the empty set.
pub fn records_listed_by<'a, R: ListedRecord + 'a>(
    records: impl Iterator<Item = &'a R> + 'a,
    list_by: Option<&str>,
    category: Option<&'a str>,
) -> impl Iterator<Item = &'a R> + 'a {
    let wanted_built_in = list_by == Some(LIST_BY_SYSTEM);
    records.filter(move |record| {
        record.is_system() == wanted_built_in
            && category.is_none_or(|want| record.record_category() == Some(want))
    })
}

/// The page of `queues` `ListQueues` reports, ordered on the key `list_by` names in the
/// direction `order` names, through [`page_of_records`].
///
/// `QueueListBy`: "you can choose to list them alphabetically by NAME or chronologically
/// by CREATION_DATE. If you don't specify, the service will list them by creation date" —
/// so this listing's default key is the creation-date one, whose oldest record is the
/// account's built-in `Default` queue. Everything else about the page is
/// [`page_of_records`]'s.
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to.
pub fn page_of_queues<'a>(
    queues: impl Iterator<Item = &'a QueueRecord>,
    list_by: Option<&str>,
    order: Option<&str>,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> Page<'a, QueueRecord> {
    page_of_records(
        queues,
        ListBySortKey::from_list_by(list_by, ListBySortKey::CreationDate),
        order,
        ORDER_ASCENDING,
        resume_from,
        max_results,
    )
}

/// The page of `presets` `ListPresets` reports, ordered on the key `list_by` names in the
/// direction `order` names, through [`page_of_records`].
///
/// `PresetListBy`: "you can choose to list them alphabetically by NAME or chronologically
/// by CREATION_DATE. If you don't specify, the service will list them by name" — so this
/// listing's default key is the NAME one, which is the only way it differs from the queue
/// listing.
///
/// `presets` is whatever set was selected (a `Type`, a category), which is
/// [`records_listed_by`]'s business; ordering and paging it is [`page_of_records`]'s.
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to.
pub fn page_of_presets<'a>(
    presets: impl Iterator<Item = &'a PresetRecord>,
    list_by: Option<&str>,
    order: Option<&str>,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> Page<'a, PresetRecord> {
    page_of_records(
        presets,
        ListBySortKey::from_list_by(list_by, ListBySortKey::Name),
        order,
        ORDER_ASCENDING,
        resume_from,
        max_results,
    )
}

/// The page of job templates `ListJobTemplates` reports, ordered on the key `list_by`
/// names in the direction `order` names, through [`page_of_records`].
///
/// `JobTemplateListBy`: "you can choose to list them alphabetically by NAME or
/// chronologically by CREATION_DATE. If you don't specify, the service will list them by
/// name" — so this listing's default key is the NAME one, as the preset listing's is.
///
/// `job_templates` is whatever set was selected (a `Type`, a category), which is
/// [`records_listed_by`]'s business; ordering and paging it is [`page_of_records`]'s.
///
/// `resume_from` is the ordering key a caller's `nextToken` decoded to.
pub fn page_of_job_templates<'a>(
    job_templates: impl Iterator<Item = &'a JobTemplateRecord>,
    list_by: Option<&str>,
    order: Option<&str>,
    resume_from: Option<&(u128, String)>,
    max_results: Option<i32>,
) -> Page<'a, JobTemplateRecord> {
    page_of_records(
        job_templates,
        ListBySortKey::from_list_by(list_by, ListBySortKey::Name),
        order,
        ORDER_ASCENDING,
        resume_from,
        max_results,
    )
}
