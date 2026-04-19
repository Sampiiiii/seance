// Owns startup log initialization, log file routing, and panic capture for the app binary.

use std::{
    backtrace::Backtrace,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Write},
    panic::{self, PanicHookInfo},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Context, Result, anyhow};
use seance_core::AppPaths;
use seance_observability::RENDER_TRACE_TARGET;
use time::OffsetDateTime;
use tracing_subscriber::EnvFilter;

const TRACE_FILTER: &str = "seance_app=trace,seance_core=trace,seance_ui=trace,seance_terminal=trace,seance_ssh=trace,seance_platform=trace,warn";
const DEFAULT_FILTER: &str =
    "seance_app=info,seance_core=info,seance_ui=info,seance_terminal=info,seance_ssh=info,warn";
static PANIC_SINK: OnceLock<Arc<PanicSink>> = OnceLock::new();

#[derive(Clone, Debug)]
pub(crate) struct DiagnosticsHandle {
    log_path: PathBuf,
    filter: String,
}

impl DiagnosticsHandle {
    pub(crate) fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub(crate) fn filter(&self) -> &str {
        &self.filter
    }
}

#[derive(Default)]
struct PanicSink {
    file: Mutex<Option<Arc<Mutex<File>>>>,
}

impl PanicSink {
    fn attach_file(&self, file: Arc<Mutex<File>>) {
        if let Ok(mut slot) = self.file.lock() {
            *slot = Some(file);
        }
    }

    fn write_line(&self, line: &str) {
        if let Ok(slot) = self.file.lock()
            && let Some(file) = slot.as_ref()
            && let Ok(mut file) = file.lock()
        {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

#[derive(Clone)]
struct SharedWriter {
    file: Arc<Mutex<File>>,
    stderr_enabled: bool,
}

impl SharedWriter {
    fn new(file: Arc<Mutex<File>>) -> Self {
        Self {
            file,
            stderr_enabled: io::stderr().is_terminal(),
        }
    }
}

struct TeeWriter {
    shared: SharedWriter,
}

impl TeeWriter {
    fn new(shared: SharedWriter) -> Self {
        Self { shared }
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(mut file) = self.shared.file.lock() {
            file.write_all(buf)?;
            file.flush()?;
        }

        if self.shared.stderr_enabled {
            let mut stderr = io::stderr().lock();
            stderr.write_all(buf)?;
            stderr.flush()?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Ok(mut file) = self.shared.file.lock() {
            file.flush()?;
        }

        if self.shared.stderr_enabled {
            io::stderr().lock().flush()?;
        }

        Ok(())
    }
}

pub(crate) fn install_panic_hook() {
    let sink = PANIC_SINK
        .get_or_init(|| Arc::new(PanicSink::default()))
        .clone();

    panic::set_hook(Box::new(move |info| {
        let rendered = render_panic(info);
        sink.write_line(&rendered);
        eprint!("{rendered}");
    }));
}

pub(crate) fn initialize(paths: &AppPaths) -> Result<DiagnosticsHandle> {
    let log_dir = resolve_log_dir(paths);
    fs::create_dir_all(&log_dir).with_context(|| {
        format!(
            "failed to create diagnostics log directory at {}",
            log_dir.display()
        )
    })?;

    let log_path = log_dir.join(log_file_name(OffsetDateTime::now_utc())?);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open diagnostics log file {}", log_path.display()))?;
    let file = Arc::new(Mutex::new(file));

    if let Some(sink) = PANIC_SINK.get() {
        sink.attach_file(Arc::clone(&file));
    }

    let filter = select_filter(
        env::var("RUST_LOG").ok().as_deref(),
        env::var("SEANCE_TRACE").ok().as_deref(),
        env::var("SEANCE_RENDER_TRACE").ok().as_deref(),
    );
    let writer = SharedWriter::new(file);
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter.clone()))
        .with_writer(move || TeeWriter::new(writer.clone()))
        .with_ansi(false)
        .with_target(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .try_init()
        .map_err(|error| anyhow!("failed to initialize tracing subscriber: {error}"))?;

    Ok(DiagnosticsHandle { log_path, filter })
}

fn log_file_name(now: OffsetDateTime) -> Result<String> {
    let format = time::format_description::parse("[year][month][day]-[hour][minute][second]")
        .context("failed to build diagnostics log timestamp format")?;
    let timestamp = now
        .format(&format)
        .context("failed to format diagnostics log timestamp")?;
    Ok(format!("launch-{timestamp}.log"))
}

fn render_panic(info: &PanicHookInfo<'_>) -> String {
    let current_thread = std::thread::current();
    let thread_name = current_thread.name().unwrap_or("<unnamed>").to_string();
    let payload = panic_message(info);
    let location = info
        .location()
        .map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        })
        .unwrap_or_else(|| "<unknown>".to_string());
    let backtrace = Backtrace::force_capture();

    format!(
        "panic: thread={thread_name} location={location} payload={payload}\nbacktrace:\n{backtrace}\n"
    )
}

fn panic_message(info: &PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn resolve_log_dir(paths: &AppPaths) -> PathBuf {
    resolve_log_dir_with_override(
        &paths.diagnostics_dir,
        env::var_os("SEANCE_LOG_DIR").as_deref(),
    )
}

fn resolve_log_dir_with_override(default_dir: &Path, override_dir: Option<&OsStr>) -> PathBuf {
    override_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_dir.to_path_buf())
}

fn select_filter(
    rust_log: Option<&str>,
    seance_trace: Option<&str>,
    seance_render_trace: Option<&str>,
) -> String {
    if let Some(filter) = rust_log
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    {
        return filter;
    }

    let mut filter = if env_flag_enabled(seance_trace) {
        TRACE_FILTER.to_string()
    } else {
        DEFAULT_FILTER.to_string()
    };

    if env_flag_enabled(seance_render_trace) {
        filter.push(',');
        filter.push_str(RENDER_TRACE_TARGET);
        filter.push_str("=trace");
    }

    filter
}

fn env_flag_enabled(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "yes" | "YES"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{DEFAULT_FILTER, TRACE_FILTER, resolve_log_dir_with_override, select_filter};
    use seance_observability::RENDER_TRACE_TARGET;

    #[test]
    fn log_dir_defaults_to_app_paths_directory() {
        let default = Path::new("/tmp/seance/logs");

        assert_eq!(
            resolve_log_dir_with_override(default, None),
            default.to_path_buf()
        );
    }

    #[test]
    fn log_dir_uses_override_when_present() {
        let default = Path::new("/tmp/seance/logs");
        let override_dir = Path::new("/tmp/custom-seance-logs");

        assert_eq!(
            resolve_log_dir_with_override(default, Some(override_dir.as_os_str())),
            override_dir.to_path_buf()
        );
    }

    #[test]
    fn rust_log_takes_precedence_over_seance_trace() {
        assert_eq!(
            select_filter(Some("seance_app=debug"), Some("1"), Some("1")),
            "seance_app=debug"
        );
    }

    #[test]
    fn seance_trace_enables_trace_defaults() {
        assert_eq!(select_filter(None, Some("1"), None), TRACE_FILTER);
    }

    #[test]
    fn render_trace_appends_dedicated_target() {
        assert_eq!(
            select_filter(None, None, Some("1")),
            format!("{DEFAULT_FILTER},{RENDER_TRACE_TARGET}=trace")
        );
    }

    #[test]
    fn missing_env_uses_info_defaults() {
        assert_eq!(select_filter(None, None, None), DEFAULT_FILTER);
    }
}
