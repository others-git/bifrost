//! The in-app event journal — dev-mode debugging without touching `RUST_LOG`.
//!
//! A [`tracing_subscriber::Layer`] captures every `bifrost::*` event at debug
//! level or louder into a bounded in-memory ring buffer, no matter what the
//! console filter is set to. The dev-mode API (`GET /api/dev/events`) reads it
//! and Settings → Developer renders it live, so "why didn't my automation
//! fire?" is answerable from the UI: the same records the server logs —
//! automation fires and skip reasons (`bifrost::automation`), the voice
//! pipeline (`bifrost::voice`), device state pushes (`bifrost::events`),
//! discovery (`bifrost::discover`), composite routing (`bifrost::composite`) —
//! are all in the buffer.
//!
//! The journal is a process-wide static, like the tracing subscriber that
//! feeds it: recording is a lock-push of a few small strings (~1000-entry cap),
//! cheap enough to run unconditionally; dev mode gates *reading*, not
//! collection.

use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use tracing::field::{Field, Visit};

/// How many entries the ring buffer keeps before dropping the oldest.
const CAPACITY: usize = 1000;

/// One captured event.
#[derive(Debug, Clone, Serialize)]
pub struct JournalEntry {
    /// Monotonic sequence number — clients poll with `after=<last seen>`.
    pub seq: u64,
    /// RFC 3339 UTC timestamp.
    pub ts: String,
    pub level: &'static str,
    /// The tracing target (`bifrost::automation`, `bifrost::voice`, …).
    pub target: String,
    pub message: String,
    /// The event's structured fields, stringified.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

/// The bounded event buffer.
#[derive(Default)]
pub struct Journal {
    entries: Mutex<VecDeque<JournalEntry>>,
    seq: AtomicU64,
}

impl Journal {
    /// The process-wide journal the layer writes to and the dev API reads.
    pub fn global() -> &'static Journal {
        static JOURNAL: OnceLock<Journal> = OnceLock::new();
        JOURNAL.get_or_init(Journal::default)
    }

    /// Append an entry, dropping the oldest past capacity.
    pub fn record(
        &self,
        level: &'static str,
        target: &str,
        message: String,
        fields: BTreeMap<String, String>,
    ) {
        let entry = JournalEntry {
            seq: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level,
            target: target.to_string(),
            message,
            fields,
        };
        let mut buf = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if buf.len() >= CAPACITY {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Entries with `seq > after`, optionally filtered to a target prefix and
    /// a minimum severity, oldest first, capped at `limit`. Also returns the
    /// newest seq overall so a filtered poll still advances its cursor past
    /// non-matching entries.
    pub fn entries_after(
        &self,
        after: u64,
        target_prefix: Option<&str>,
        min_level: Option<&str>,
        limit: usize,
    ) -> (Vec<JournalEntry>, u64) {
        let floor = min_level.map(level_rank);
        let buf = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let last_seq = buf
            .back()
            .map_or(self.seq.load(Ordering::Relaxed), |e| e.seq);
        let entries = buf
            .iter()
            .filter(|e| e.seq > after)
            .filter(|e| target_prefix.is_none_or(|p| e.target.starts_with(p)))
            .filter(|e| floor.is_none_or(|f| level_rank(e.level) >= f))
            .take(limit)
            .cloned()
            .collect();
        (entries, last_seq)
    }

    /// Every distinct target currently in the buffer, sorted. This is what the
    /// panel's area filter offers: a hand-maintained list drifts the moment a
    /// new `tracing` target appears (and misses the ones that inherit their
    /// module path instead of naming a target explicitly), so the options are
    /// read from what the server has actually emitted. Computed over the WHOLE
    /// buffer, never the filtered view, so picking an area doesn't collapse the
    /// menu to that one choice.
    pub fn areas(&self) -> Vec<String> {
        let buf = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        // Dedupe by reference, clone only the survivors: the panel polls this
        // every 2s and the buffer holds ~1000 entries that collapse to a dozen
        // targets — and the lock is the one the capture layer needs per event.
        buf.iter()
            .map(|e| e.target.as_str())
            .collect::<std::collections::BTreeSet<_>>() // deduped and sorted
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    /// Drop everything (the panel's Clear button).
    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

/// Severity order for the panel's level filter ("warnings and up", "errors
/// only"). Unknown strings rank lowest, so they never sneak past a floor.
fn level_rank(level: &str) -> u8 {
    match level.to_ascii_uppercase().as_str() {
        "ERROR" => 4,
        "WARN" => 3,
        "INFO" => 2,
        "DEBUG" => 1,
        _ => 0, // TRACE + anything unexpected
    }
}

// ── The capture layer ─────────────────────────────────────────────────────────

/// A tracing layer feeding the global journal. Pair it with a `Targets` filter
/// for `bifrost` at DEBUG (see `lib.rs`) so it captures Bifrost's own events
/// regardless of the console `RUST_LOG` filter.
pub struct JournalLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for JournalLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let meta = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        Journal::global().record(
            meta.level().as_str(),
            meta.target(),
            visitor.message,
            visitor.fields,
        );
    }
}

/// Collects an event's fields as strings; `message` is split out.
#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_report_every_target_actually_emitted() {
        let j = Journal::default();
        assert!(
            j.areas().is_empty(),
            "nothing emitted yet, nothing to offer"
        );
        j.record("DEBUG", "bifrost::smarttv", "a".into(), BTreeMap::new());
        j.record("INFO", "bifrost::connection", "b".into(), BTreeMap::new());
        j.record("DEBUG", "bifrost::smarttv", "c".into(), BTreeMap::new());
        // Deduped and sorted — this is the panel's filter menu, so it must
        // reflect what the log rows show, including module-path targets that
        // never name themselves explicitly.
        assert_eq!(j.areas(), vec!["bifrost::connection", "bifrost::smarttv"]);
        j.clear();
        assert!(j.areas().is_empty());
    }

    #[test]
    fn ring_buffer_caps_and_sequences() {
        let j = Journal::default();
        for i in 0..(CAPACITY + 10) {
            j.record("DEBUG", "bifrost::test", format!("m{i}"), BTreeMap::new());
        }
        let (entries, last_seq) = j.entries_after(0, None, None, usize::MAX);
        assert_eq!(entries.len(), CAPACITY);
        // Oldest 10 dropped; sequence still monotonic and gap-free at the tail.
        assert_eq!(entries.first().unwrap().seq, 11);
        assert_eq!(last_seq, (CAPACITY + 10) as u64);
    }

    #[test]
    fn entries_after_honors_a_minimum_level() {
        let j = Journal::default();
        j.record("DEBUG", "bifrost::a", "noise".into(), BTreeMap::new());
        j.record("WARN", "bifrost::a", "wobble".into(), BTreeMap::new());
        j.record("ERROR", "bifrost::a", "broken".into(), BTreeMap::new());

        let (warn_up, _) = j.entries_after(0, None, Some("warn"), usize::MAX);
        assert_eq!(warn_up.len(), 2, "warn floor keeps WARN + ERROR");

        let (errors, last) = j.entries_after(0, None, Some("error"), usize::MAX);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "broken");
        // The cursor still advances past filtered-out entries.
        assert_eq!(last, 3);

        let (unknown_floor, _) = j.entries_after(0, None, Some("bogus"), usize::MAX);
        assert_eq!(unknown_floor.len(), 3, "an unknown floor filters nothing");
    }

    #[test]
    fn entries_after_filters_by_cursor_and_target_prefix() {
        let j = Journal::default();
        j.record("DEBUG", "bifrost::voice", "heard".into(), BTreeMap::new());
        j.record(
            "DEBUG",
            "bifrost::automation",
            "fired".into(),
            BTreeMap::new(),
        );
        j.record("INFO", "bifrost::voice", "spoke".into(), BTreeMap::new());

        let (all, last) = j.entries_after(0, None, None, usize::MAX);
        assert_eq!(all.len(), 3);
        assert_eq!(last, 3);

        let (voice, last) = j.entries_after(0, Some("bifrost::voice"), None, usize::MAX);
        assert_eq!(voice.len(), 2);
        // The cursor still reports the global tail so polls advance past
        // non-matching entries.
        assert_eq!(last, 3);

        let (after, _) = j.entries_after(2, None, None, usize::MAX);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].message, "spoke");
    }

    #[test]
    fn layer_captures_events_with_fields() {
        use tracing_subscriber::prelude::*;
        // A scoped subscriber writing into the global journal.
        let subscriber = tracing_subscriber::registry().with(JournalLayer);
        let before = Journal::global().entries_after(0, None, None, usize::MAX).1;
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(target: "bifrost::journal_test", device = "l1", on = true, "state push");
        });
        let (entries, _) = Journal::global().entries_after(
            before,
            Some("bifrost::journal_test"),
            None,
            usize::MAX,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "state push");
        assert_eq!(
            entries[0].fields.get("device").map(String::as_str),
            Some("l1")
        );
        assert_eq!(
            entries[0].fields.get("on").map(String::as_str),
            Some("true")
        );
        assert_eq!(entries[0].level, "DEBUG");
    }
}
