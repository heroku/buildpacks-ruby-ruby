use crate::output::background_timer::{start_timer, StopJoinGuard, StopTimer};
use crate::output::fmt;
use std::fmt::Debug;
use std::io::{stdout, Stdout, Write};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[allow(clippy::wildcard_imports)]
use crate::output::interface::*;

/// # Build output logging
///
/// The main interfaces are `BuildLog` and `LayerLogger`
///
/// ## `BuildLog`
///
/// TODO
///

#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct BuildLog<T, W: Debug> {
    io: W,
    state: PhantomData<T>,
    started: Instant,
}

impl<W> StoppedLogger for BuildLog<state::Stopped, W> where W: Debug {}

impl<W> Logger for BuildLog<state::NotStarted, W>
where
    W: Write + Send + Sync + Debug + 'static,
{
    fn buildpack_name(mut self, buildpack_name: &str) -> Box<dyn StartedLogger> {
        write_now(&mut self.io, format!("{}\n\n", fmt::header(buildpack_name)));

        Box::new(BuildLog {
            io: self.io,
            state: PhantomData::<state::Started>,
            started: self.started,
        })
    }

    fn without_buildpack_name(self) -> Box<dyn StartedLogger> {
        Box::new(BuildLog {
            io: self.io,
            state: PhantomData::<state::Started>,
            started: self.started,
        })
    }
}

impl<W> BuildLog<state::NotStarted, W>
where
    W: Write + Debug,
{
    pub fn new(io: W) -> Self {
        Self {
            io,
            state: PhantomData::<state::NotStarted>,
            started: Instant::now(),
        }
    }
}

impl BuildLog<state::NotStarted, Stdout> {
    #[allow(dead_code)]
    fn to_stdout() -> Self {
        Self {
            io: stdout(),
            state: PhantomData::<state::NotStarted>,
            started: Instant::now(),
        }
    }
}

impl BuildLog<state::NotStarted, std::fs::File> {
    #[allow(dead_code)]
    fn to_file(path: &std::path::Path) -> Result<Self, std::io::Error> {
        Ok(Self {
            io: fs_err::File::create(path)?.into(),
            state: PhantomData::<state::NotStarted>,
            started: Instant::now(),
        })
    }
}

impl<W> StartedLogger for BuildLog<state::Started, W>
where
    W: Write + Send + Sync + Debug + 'static,
{
    fn section(mut self: Box<Self>, s: &str) -> Box<dyn SectionLogger> {
        writeln_now(&mut self.io, fmt::section(s));

        Box::new(BuildLog {
            io: self.io,
            state: PhantomData::<state::InSection>,
            started: self.started,
        })
    }

    fn finish_logging(mut self: Box<Self>) -> Box<dyn StoppedLogger> {
        let elapsed = fmt::time::human(&self.started.elapsed());
        let details = fmt::details(format!("finished in {elapsed}"));

        writeln_now(&mut self.io, fmt::section(format!("Done {details}")));

        Box::new(BuildLog {
            io: self.io,
            state: PhantomData::<state::Stopped>,
            started: self.started,
        })
    }
}

fn write_now<D: Write>(destination: &mut D, msg: impl AsRef<str>) {
    write!(destination, "{}", msg.as_ref()).expect("Internal error: UI writer closed");

    destination
        .flush()
        .expect("Internal error: UI writer closed");
}

fn writeln_now<D: Write>(destination: &mut D, msg: impl AsRef<str>) {
    writeln!(destination, "{}", msg.as_ref()).expect("Internal error: UI writer closed");

    destination
        .flush()
        .expect("Internal error: UI writer closed");
}

#[derive(Debug)]
struct StreamTimed<W> {
    arc_io: Arc<Mutex<W>>,
    started: Instant,
    build_timer: Instant,
}

struct LockedWriter<W> {
    arc: Arc<Mutex<W>>,
}

impl<W> Write for LockedWriter<W>
where
    W: Write + Send + Sync + Debug + 'static,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut io = self.arc.lock().expect("Internal error");
        io.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut io = self.arc.lock().expect("Internal error");
        io.flush()
    }
}

impl<W> StreamTimed<W>
where
    W: Write + Send + Sync + Debug,
{
    fn start(&mut self) {
        let mut guard = self.arc_io.lock().expect("Internal error");
        let mut io = guard.by_ref();
        // Newline before stream
        writeln_now(&mut io, "");
    }
}

impl<W> StreamLogger for StreamTimed<W>
where
    W: Write + Send + Sync + Debug + 'static,
{
    fn io(&mut self) -> Box<dyn Write + Send + Sync> {
        Box::new(libherokubuildpack::write::line_mapped(
            LockedWriter {
                arc: self.arc_io.clone(),
            },
            fmt::cmd_stream_format,
        ))
    }

    fn finish_timed_stream(self: Box<Self>) -> Box<dyn SectionLogger> {
        let duration = self.started.elapsed();
        let mut io = Arc::try_unwrap(self.arc_io)
            .expect("Internal error")
            .into_inner()
            .expect("Internal error");

        // Newline after stream
        writeln_now(&mut io, "");

        let mut section = BuildLog {
            io,
            state: PhantomData::<state::InSection>,
            started: self.build_timer,
        };

        section.step(&format!(
            "Done {}",
            fmt::details(fmt::time::human(&duration))
        ));

        Box::new(section)
    }
}

#[derive(Debug)]
struct FinishTimedStep<W> {
    arc_io: Arc<Mutex<W>>,
    background: StopJoinGuard<StopTimer>,
    build_timer: Instant,
}

impl<W> TimedStepLogger for FinishTimedStep<W>
where
    W: Write + Send + Sync + Debug + 'static,
{
    fn finish_timed_step(self: Box<Self>) -> Box<dyn SectionLogger> {
        // Must stop background writing thread before retrieving IO
        let duration = self.background.stop().expect("Internal error").elapsed();

        let mut io = Arc::try_unwrap(self.arc_io)
            .expect("Internal error")
            .into_inner()
            .expect("Internal error");

        let contents = fmt::details(fmt::time::human(&duration));
        write_now(&mut io, format!("{contents}\n"));

        Box::new(BuildLog {
            io,
            state: PhantomData::<state::InSection>,
            started: self.build_timer,
        })
    }
}

impl<W> SectionLogger for BuildLog<state::InSection, W>
where
    W: Write + Send + Sync + Debug + 'static,
{
    fn step(&mut self, s: &str) {
        writeln_now(&mut self.io, fmt::step(s));
    }

    fn step_and(mut self: Box<Self>, s: &str) -> Box<dyn SectionLogger> {
        self.step(s);

        Box::new(BuildLog {
            io: self.io,
            state: PhantomData::<state::InSection>,
            started: self.started,
        })
    }

    fn step_timed(self: Box<Self>, s: &str) -> Box<dyn TimedStepLogger> {
        let start = fmt::step(format!("{s}{}", fmt::background_timer_start()));
        let tick = fmt::background_timer_tick();
        let end = fmt::background_timer_end();

        let arc_io = Arc::new(Mutex::new(self.io));
        let background = start_timer(&arc_io, start, tick, end);

        Box::new(FinishTimedStep {
            arc_io,
            background,
            build_timer: self.started,
        })
    }

    fn step_timed_stream(mut self: Box<Self>, s: &str) -> Box<dyn StreamLogger> {
        self.step(s);

        let started = Instant::now();
        let build_timer = self.started;
        let arc_io = Arc::new(Mutex::new(self.io));
        let mut stream = StreamTimed {
            arc_io,
            started,
            build_timer,
        };
        stream.start();

        Box::new(stream)
    }

    fn end_section(self: Box<Self>) -> Box<dyn StartedLogger> {
        Box::new(BuildLog {
            io: self.io,
            state: PhantomData::<state::Started>,
            started: self.started,
        })
    }
}

impl<T, W> ErrorWarningImportantLogger for BuildLog<T, W>
where
    T: Debug,
    W: Write + Debug,
{
    fn warning(&mut self, s: &str) {
        writeln_now(&mut self.io, fmt::warn(s));
    }

    fn important(&mut self, s: &str) {
        writeln_now(&mut self.io, fmt::important(s));
    }
}

impl<T, W> ErrorLogger for BuildLog<T, W>
where
    T: Debug,
    W: Write + Debug,
{
    fn error(&mut self, s: &str) {
        writeln_now(&mut self.io, fmt::error(s));
    }
}

mod state {
    #[derive(Debug)]
    pub struct NotStarted;

    #[derive(Debug)]
    pub struct Started;

    #[derive(Debug)]
    pub struct InSection;

    #[derive(Debug)]
    pub struct Stopped;
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::output::fmt;
    use crate::output::util::{LinesWithEndings, ReadYourWrite};
    use indoc::formatdoc;
    use libherokubuildpack::command::CommandExt;

    #[test]
    fn test_captures() {
        let writer = ReadYourWrite::writer(Vec::new());
        let reader = writer.reader();

        let mut stream = BuildLog::new(writer)
            .buildpack_name("Heroku Ruby Buildpack")
            .section("Ruby version `3.1.3` from `Gemfile.lock`")
            .step_timed("Installing")
            .finish_timed_step()
            .end_section()
            .section("Hello world")
            .step_timed_stream("Running `echo 'hello world'`");

        std::process::Command::new("echo")
            .arg("hello world")
            .output_and_write_streams(stream.io(), stream.io())
            .unwrap();

        stream.finish_timed_stream().end_section().finish_logging();

        let actual = fmt::strip_control_codes(String::from_utf8_lossy(&reader.lock().unwrap()));

        let actual = LinesWithEndings::from(&actual)
            .map(|line| {
                // Remove empty indented lines https://github.com/heroku/libcnb.rs/issues/582
                regex::Regex::new(r#"^\s+$"#)
                    .expect("clippy")
                    .replace(line, "\n")
                    .to_string()
            })
            .collect::<String>();

        let expected = formatdoc! {"

            # Heroku Ruby Buildpack

            - Ruby version `3.1.3` from `Gemfile.lock`
              - Installing ... (< 0.1s)
            - Hello world
              - Running `echo 'hello world'`

                  hello world

              - Done (< 0.1s)
            - Done (finished in < 0.1s)
        "};

        assert_eq!(expected, actual);
    }
}
