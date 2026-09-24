//! The subscriber: what is recorded, how it reads, and where it goes.
//!
//! Two sinks, one filter each, built from injected parts so that everything
//! except the wiring can be tested without a running application:
//!
//! - the **file** -- `rolling::Writer` over the log directory, behind the mask;
//! - **stderr** -- always attached. The ruling is that a log directory which
//!   cannot be written must not stop a launch, and the answer to "then where
//!   does it go" has to be a real answer: the terminal, as before this CHG.
//!
//! The mask is applied by the sink, on the finished line, which is the one
//! place every record passes through whatever wrote it -- the Agent learned the
//! same lesson the hard way (a `Filter` cannot reach a traceback, so its mask
//! lives in the formatter). Its `secrets` are the values this process holds and
//! no key names, above all the launch token.

use std::io::{self, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::layer::{Identity, Layer, SubscriberExt};
use tracing_subscriber::registry::{LookupSpan, Registry};

use crate::logging::{redact, rolling, targets::Levels};

/// Where the formatter reads the time.
///
/// It lives here rather than in `rolling` because rotation time is no longer
/// ours to read: `file-rotate` takes its own clock, and (measured) does not let a
/// caller supply one — its `mock_time` is `#[cfg(test)]` inside the crate. What
/// is left to inject is the **stamp**, which is why this trait is one method
/// wide and why the tests that need a fixed second still get one.
pub trait Clock {
    fn now(&self) -> SystemTime;
}

/// The system clock, which is what a launch uses.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// What the subscriber needs, all of it injected.
pub struct Options {
    /// Which targets are recorded, and at what level.
    pub levels: Levels,
    /// Values this process holds and must not print. The launch token arrives
    /// named by nobody, so no shape rule would catch it.
    pub secrets: Vec<String>,
    /// Where the file goes. `None` is a launch whose log directory could not be
    /// prepared: stderr only, and the program still starts.
    pub directory: Option<PathBuf>,
    /// How long history is kept and how long one record may be (`[logging]`,
    /// T-14).
    pub limits: rolling::Limits,
    /// The clock the stamp is read from. The file's own rotation reads the same
    /// wall clock from inside the crate, which is why the stamp has to agree
    /// with it in *zone* and not only in instant: see [`stamp`].
    pub clock: Arc<dyn Clock + Send + Sync>,
}

/// Build the subscriber. It is not installed here: the caller owns the
/// process-wide decision (`set_global_default`), which a test must not make.
///
/// Nothing in here fails. A directory that cannot be written is refused by
/// `rolling::Writer::open` as a value, and the ruling is that it may not stop a
/// launch -- so the failure surfaces on the sink that is still working, not as
/// an error return the caller would have to invent a policy for.
pub fn assemble(options: Options) -> impl Subscriber + Send + Sync {
    let filter = options.levels.filter();
    let format = LineFormat {
        clock: Arc::clone(&options.clock),
    };
    let secrets: Arc<[String]> = options.secrets.into();

    let document: Box<dyn Layer<Registry> + Send + Sync> = match options.directory.as_ref() {
        Some(directory) => {
            match rolling::Writer::open(directory, options.limits) {
                Ok(writer) => Box::new(
                    tracing_subscriber::fmt::layer()
                        .event_format(format.clone())
                        .with_writer(LineWriter::file(writer, Arc::clone(&secrets)))
                        .with_filter(filter.clone()),
                ),
                // Reached only if the directory cannot be used at all. The
                // records still have somewhere to go.
                Err(reason) => {
                    note(&reason.to_string());
                    Box::new(Identity::default())
                }
            }
        }
        None => Box::new(Identity::default()),
    };

    let terminal = tracing_subscriber::fmt::layer()
        .event_format(format)
        .with_writer(LineWriter::stderr(secrets))
        .with_filter(filter);

    Registry::default().with(document).with(terminal)
}

/// The record as one plain-text line, in the Agent's shape:
///
/// ```text
/// 2026-09-24T18:11:12 [INFO] desktop.startup: the launch summary
/// ```
///
/// The shape is `logging.py`'s `FMT`/`DATEFMT`, so a reader moving between the
/// two halves of the product reads the same line. One deliberate difference
/// remains, registered in the CHG evidence:
///
/// - the fields carry no span context. Desktop uses no spans (the `attributes`
///   feature is off and nothing here opens one), so there is none to lose; a
///   future span would have to come back to this line and say how to print it.
///
/// The stamp used to be UTC with a `Z` (CHG-057 T-12), and that is superseded by
/// CHG-058's ruling: the archive names come from `file-rotate`, which formats
/// `chrono::Local` and offers no UTC switch, so a UTC stamp would disagree with
/// the name of the file it sits in for eight hours a day. Both sides of the
/// product now stamp and name in local time -- which is what the Agent always
/// did, so this change *removes* the last difference rather than adding one.
#[derive(Clone)]
struct LineFormat {
    clock: Arc<dyn Clock + Send + Sync>,
}

impl<S, N> FormatEvent<S, N> for LineFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let mut body = String::new();
        ctx.field_format()
            .format_fields(Writer::new(&mut body), event)?;
        let metadata = event.metadata();
        writeln!(
            writer,
            "{} [{}] {}: {}",
            stamp(self.clock.now()),
            metadata.level(),
            metadata.target(),
            single_line(&body)
        )
    }
}

/// The stamp, `%Y-%m-%dT%H:%M:%S`, in **local** time and unmarked.
///
/// The Agent's `%(asctime)s` is local and unmarked, and the archive names are
/// local (see [`LineFormat`]), so this is the shape that agrees with both. It is
/// `rolling::STAMP_FORMAT` and not a literal because the archive-name format
/// beside it is the other half of the same agreement: the two must spell the
/// same clock.
fn stamp(time: SystemTime) -> String {
    stamp_with(rolling::STAMP_FORMAT, time)
}

/// The same clock, in whatever format a caller needs.
///
/// One function for "a moment, in the machine's own time zone" so that the three
/// stamps this product writes — a log line's, an archive name's, a diagnostic
/// bundle's — cannot end up on three different clocks. They are different
/// formats of one reading, which is exactly the property an operator relies on
/// when they line up a bundle beside the file it came from.
pub(crate) fn stamp_with(format: &str, time: SystemTime) -> String {
    let local: chrono::DateTime<chrono::Local> = time.into();
    local.format(format).to_string()
}

/// One record, one line.
///
/// A message is free to contain a newline -- a webview's stack trace does
/// (T-16) -- and the file must not, or a reader counting lines counts records
/// that were never written. Escaped rather than cut, so the break is still
/// visible, in the two escapes the Agent's `_field` uses.
fn single_line(text: &str) -> String {
    if !text.contains(['\r', '\n']) {
        return text.to_owned();
    }
    text.replace('\r', "\\r").replace('\n', "\\n")
}

/// A sink for one finished line, already masked.
#[derive(Clone)]
enum Sink {
    /// The file, shared: one `rolling::Writer` per launch, and a `MakeWriter`
    /// per record.
    File(Arc<Mutex<rolling::Writer>>),
    /// The terminal, always available.
    Stderr,
}

impl Sink {
    fn write_line(&self, line: &str) {
        match self {
            Sink::File(inner) => match inner.lock() {
                Ok(mut writer) => {
                    if let Err(reason) = writer.write_line(line) {
                        note(&reason.to_string());
                    }
                }
                // Another thread panicked while holding the writer. Locking it
                // again would repeat that panic on this thread, and the record
                // is on stderr already; saying so is what is left.
                Err(_) => note("the log writer was poisoned by an earlier panic"),
            },
            Sink::Stderr => {
                let _ = writeln!(io::stderr(), "{line}");
            }
        }
    }
}

/// Say something about logging without logging it.
///
/// The one channel that cannot be the broken one, and never the record itself:
/// the line is already on stderr, and what a reader of an empty log file needs
/// is the reason it is empty. `setup` uses the same channel for the failures
/// that precede the subscriber — one place, so "why is the log empty" has one
/// answer to grep for.
pub(crate) fn note(what: &str) {
    let _ = writeln!(io::stderr(), "desktop.log: {what}");
}

/// The line in, the writer out: mask, then write.
#[derive(Clone)]
struct LineWriter {
    sink: Sink,
    secrets: Arc<[String]>,
}

impl LineWriter {
    fn file(writer: rolling::Writer, secrets: Arc<[String]>) -> LineWriter {
        LineWriter {
            sink: Sink::File(Arc::new(Mutex::new(writer))),
            secrets,
        }
    }

    fn stderr(secrets: Arc<[String]>) -> LineWriter {
        LineWriter {
            sink: Sink::Stderr,
            secrets,
        }
    }

    fn sink(&self) -> LineSink {
        LineSink {
            sink: self.sink.clone(),
            secrets: Arc::clone(&self.secrets),
            buffer: Vec::new(),
        }
    }
}

impl<'a> MakeWriter<'a> for LineWriter {
    type Writer = LineSink;

    fn make_writer(&'a self) -> LineSink {
        self.sink()
    }
}

/// One record's bytes, held until the record is over.
///
/// The formatter writes a whole record -- newline and all -- in a single
/// `write_all`, so this normally buffers and emits within one call. It stays a
/// buffer rather than a pass-through because the rule it enforces is "one line
/// in, one record out": a writer that trusted its caller to send whole lines
/// would produce a second, stamp-less line the day a formatter changed, and
/// nothing would say so.
struct LineSink {
    sink: Sink,
    secrets: Arc<[String]>,
    buffer: Vec<u8>,
}

impl LineSink {
    fn emit(&self, line: &[u8]) {
        // A `\r` before the newline is a Windows line ending, not the record.
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let masked = redact::redact(&String::from_utf8_lossy(line), &self.secrets);
        self.sink.write_line(&masked);
    }
}

impl io::Write for LineSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=newline).collect();
            line.truncate(newline);
            self.emit(&line);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.emit(&line);
        }
        Ok(())
    }
}

impl Drop for LineSink {
    fn drop(&mut self) {
        // The formatter always ends its record with a newline, so this is the
        // path not taken -- kept because the alternative is dropping a record
        // in silence, and a test pins it (`a_record_that_never_got_its_newline_
        // is_still_written_when_the_sink_drops`).
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::level_filters::LevelFilter;
    use tracing::subscriber::with_default;
    use tracing::Level;

    use crate::logging::test_support::{options, scratch, written};

    use crate::config::Environment;

    /// The targets emitted below, one literal each: `target:` is expanded into
    /// a constant, so the list cannot be iterated at the call site. The
    /// comparison inside the test is what keeps the two in step -- a target
    /// added to the vocabulary without a record emitted under it fails here.
    const EMITTED_TARGETS: [&str; 3] = ["agent.supervisor", "desktop.startup", "webview"];

    /// Every stamp this product writes comes off the same clock.
    ///
    /// The property is not the format — the formats differ on purpose, and the
    /// archive names avoid `:` because a file name reads better without one. It
    /// is that the *reading* is one reading: the same `SystemTime`, converted to
    /// local time once, spelled three ways. A helper that took a different clock
    /// would put a bundle's `created_at` in a time zone its own log lines are not
    /// in, and the two are always read side by side.
    #[test]
    fn every_stamp_is_the_same_moment_the_same_way() {
        let moment = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_787_000_000);

        let line = stamp(moment);
        assert_eq!(line, stamp_with("%Y-%m-%dT%H:%M:%S", moment));
        assert_eq!(line.len(), 19, "{line}");

        let archive = stamp_with("%Y-%m-%d-%H", moment);
        assert!(
            line.starts_with(&archive[..10]),
            "the day agrees: {line} vs {archive}"
        );
        assert_eq!(
            &line[11..13],
            &archive[11..13],
            "and so does the hour: {line} vs {archive}"
        );
        assert!(!archive.contains(':'), "a file name: {archive}");
    }

    #[test]
    fn every_target_desktop_owns_produces_a_record_in_the_file() {
        // The enumerated assertion, at the level where it matters: the filter's
        // default is OFF, so a target that is not turned on by name is dropped
        // in silence.
        assert_eq!(EMITTED_TARGETS, crate::logging::targets::OWNED_TARGETS);
        let directory = scratch("owned");
        let subscriber = assemble(options(Some(&directory.0)));
        with_default(subscriber, || {
            tracing::info!(target: "agent.supervisor", "a record");
            tracing::info!(target: "desktop.startup", "a record");
            tracing::info!(target: "webview", "a record");
        });
        let text = written(&directory.0);
        for target in EMITTED_TARGETS {
            assert!(
                text.contains(&format!("[INFO] {target}: a record")),
                "{target} produced no record in:\n{text}"
            );
        }
        // One file, written once per record: the writer is shared across the
        // records rather than rebuilt per event.
        assert_eq!(text.lines().count(), EMITTED_TARGETS.len());
    }

    #[test]
    fn a_target_desktop_does_not_own_never_reaches_the_file() {
        let directory = scratch("foreign");
        let subscriber = assemble(options(Some(&directory.0)));
        with_default(subscriber, || {
            tracing::warn!(target: "h2::codec", "a connection frame");
            // The control arm: the same subscriber, the same write path, a
            // target we do own. Without it, "the frame is absent" cannot be
            // told apart from "nothing is written at all".
            tracing::info!(target: "desktop.startup", "the launch summary");
        });
        let text = written(&directory.0);
        assert!(!text.contains("a connection frame"), "{text}");
        assert!(text.contains("the launch summary"), "{text}");
    }

    #[test]
    fn the_configured_level_decides_what_reaches_the_file() {
        let quiet = scratch("quiet");
        let loud = scratch("loud");
        for (scratch, level) in [(&quiet, LevelFilter::INFO), (&loud, LevelFilter::DEBUG)] {
            let mut options = options(Some(&scratch.0));
            options.levels.default = level;
            with_default(assemble(options), || {
                tracing::debug!(target: "desktop.startup", "a detail");
                // The control: an event the level keeps, in the same record
                // stream, so a missing detail is not a missing sink.
                tracing::warn!(target: "desktop.startup", "a problem");
            });
        }
        let quiet = written(&quiet.0);
        assert!(!quiet.contains("a detail"), "{quiet}");
        assert!(quiet.contains("a problem"), "{quiet}");
        let loud = written(&loud.0);
        assert!(loud.contains("a detail"), "{loud}");
        assert!(loud.contains("a problem"), "{loud}");
    }

    #[test]
    fn supervision_is_recorded_even_at_a_level_that_would_hide_it() {
        let directory = scratch("supervision");
        let mut options = options(Some(&directory.0));
        options.levels.default = LevelFilter::ERROR;
        with_default(assemble(options), || {
            tracing::info!(target: "agent.supervisor", "the sidecar exited");
            // The control: the same INFO at a target without the floor is
            // dropped, so this is the floor and not a level that never applied.
            tracing::info!(target: "webview", "a js error");
        });
        let text = written(&directory.0);
        assert!(text.contains("the sidecar exited"), "{text}");
        assert!(!text.contains("a js error"), "{text}");
    }

    #[test]
    fn a_credential_never_reaches_the_file_and_the_line_around_it_does() {
        let secret = "Zq7Yk3Nv1PdA8sXm";
        let message = format!("connected with token={secret} to the local Agent");
        let directory = scratch("redact");
        let mut options = options(Some(&directory.0));
        options.secrets = vec![secret.to_owned()];
        with_default(assemble(options), || {
            tracing::info!(target: "desktop.startup", "{}", message);
        });
        // The control: the message really does carry the credential, so the
        // assertion below is about the file and not about the fixture.
        assert!(message.contains(secret));
        let text = written(&directory.0);
        assert!(!text.contains(secret), "{text}");
        assert!(
            text.contains("connected with token=*** to the local Agent"),
            "{text}"
        );
    }

    #[test]
    fn a_record_stays_one_line_when_the_message_does_not() {
        // A webview stack trace is the shape T-16 hands over.
        let directory = scratch("multiline");
        let subscriber = assemble(options(Some(&directory.0)));
        with_default(subscriber, || {
            tracing::warn!(
                target: "webview",
                "TypeError\n  at click (app.js:1:2)\n  at render (app.js:9:9)"
            );
        });
        let text = written(&directory.0);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(
            text.contains("TypeError\\n  at click (app.js:1:2)\\n  at render"),
            "{text}"
        );
    }

    #[test]
    fn the_line_reads_the_way_the_agent_writes_it() {
        let directory = scratch("shape");
        let subscriber = assemble(options(Some(&directory.0)));
        with_default(
            subscriber,
            || tracing::info!(target: "desktop.startup", "hello"),
        );
        assert_eq!(
            written(&directory.0),
            "2026-09-24T10:11:12 [INFO] desktop.startup: hello\n"
        );
    }

    #[test]
    fn a_record_that_never_got_its_newline_is_still_written_when_the_sink_drops() {
        // The path the current formatter never takes, pinned anyway: the
        // alternative is losing a record in silence.
        let directory = scratch("drop");
        let writer = rolling::Writer::open(&directory.0, rolling::Limits::SHIPPED).expect("open");
        {
            let mut sink = LineWriter::file(writer, Arc::from(Vec::<String>::new())).make_writer();
            io::Write::write_all(&mut sink, b"a record with no newline").expect("buffered");
        }
        assert!(written(&directory.0).contains("a record with no newline"));
    }

    /// A path that is a file, not a directory, is **refused** at `open` rather
    /// than accepted.
    ///
    /// This test used to assert the opposite — that the writer could be built
    /// over an unusable directory and that the sink then swallowed whatever
    /// happened on write. That state no longer exists: `rolling::Writer::open`
    /// asks about the directory itself, because the crate underneath would
    /// *panic* creating it and *silently drop* every later record. So what is
    /// asserted here is the refusal as a value, plus the composition the ruling
    /// actually turns on — the launch still gets a subscriber whose records go
    /// to stderr. That the record reaches stderr is T-15's real launch, because
    /// a unit test cannot read the terminal it is writing to.
    #[test]
    fn a_directory_that_cannot_be_written_is_refused_and_the_launch_still_has_a_subscriber() {
        let blocked = scratch("blocked-write");
        std::fs::write(&blocked.0, b"not a directory").expect("the scratch file");

        let refused = rolling::Writer::open(&blocked.0, rolling::Limits::SHIPPED);
        assert!(
            refused.is_err(),
            "a directory that cannot be created is refused, not accepted and then \
             written to in silence"
        );

        let subscriber = assemble(options(Some(&blocked.0)));
        with_default(
            subscriber,
            || tracing::info!(target: "desktop.startup", "still here"),
        );
        std::fs::remove_file(&blocked.0).ok();
    }

    /// The local-time stamp, which supersedes CHG-057's UTC one.
    ///
    /// The instant is built **from local fields**, so its local spelling is
    /// known by construction and the expected string is not a second reading of
    /// the same clock (which would make the test a tautology). A UTC stamp fails
    /// this on any machine that is not on UTC; on a machine that is, the `Z`
    /// assertion is what is left, and the mutation that restores `Z` is caught
    /// there too. Registered limit: a UTC-zone machine cannot tell the two
    /// apart.
    #[test]
    fn the_stamp_is_the_local_day_and_time_with_no_zone_mark() {
        use chrono::TimeZone;
        let when: SystemTime = chrono::Local
            .with_ymd_and_hms(2026, 12, 31, 23, 59, 59)
            .single()
            .expect("an unambiguous local time")
            .into();

        assert_eq!(stamp(when), "2026-12-31T23:59:59");
        assert!(
            !stamp(when).contains('Z'),
            "local time carries no zone mark: {}",
            stamp(when)
        );
        assert_eq!(
            stamp(when).len(),
            "2026-12-31T23:59:59".len(),
            "and nothing else was appended to it"
        );
    }

    /// The stamp is local at the two moments where local and UTC disagree about
    /// the **date**, which is the whole reason it moved (CHG-058 D-09).
    ///
    /// Midnight and 23:00 local: a UTC stamp puts the first in the previous day
    /// and the second in the next one. The expected strings are literals, so the
    /// implementation cannot move them -- and the format's own spelling is
    /// pinned by `rolling::tests::the_names_and_the_formats_are_the_ones_the_ruling_names`,
    /// which is where the archive-name format beside it is pinned too.
    ///
    /// Registered limit: on a machine whose zone *is* UTC this test and its
    /// sibling cannot tell the two implementations apart. Measured on the
    /// machine this was written on, where the zone is UTC+8.
    #[test]
    fn the_stamp_is_local_at_the_edges_of_the_day() {
        use chrono::TimeZone;
        for (month, day, hour) in [(1, 1, 0), (6, 30, 23), (12, 31, 23)] {
            let when: SystemTime = chrono::Local
                .with_ymd_and_hms(2026, month, day, hour, 0, 0)
                .single()
                .expect("an unambiguous local time")
                .into();
            let stamped = stamp(when);
            assert_eq!(
                &stamped[5..10],
                format!("{month:02}-{day:02}"),
                "the date is the local one: {stamped}"
            );
            assert_eq!(
                &stamped[11..13],
                format!("{hour:02}"),
                "the hour is the local one: {stamped}"
            );
        }
    }

    #[test]
    fn a_level_below_the_one_configured_is_dropped_before_it_is_rendered() {
        // `Targets` also lowers the subscriber's max-level hint, so the event
        // never reaches the formatter at all -- which is what keeps a disabled
        // level from costing anything.
        let filter = Levels::shipped(Environment::Production).filter();
        assert!(!filter.would_enable("desktop.startup", &Level::DEBUG));
    }
}
