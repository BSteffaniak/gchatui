//! Default application tracing. Only explicitly sanitized application events
//! are admitted; dependency traces may contain HTTP URLs or credentials.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub struct Logging {
    _guard: WorkerGuard,
    pub directory: PathBuf,
}

pub fn init() -> Result<Logging> {
    let directory = crate::config::default_state_dir()
        .context("cannot determine application log directory")?
        .join("logs");
    let (writer, guard) = writer(&directory)?;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                    metadata.target() == "gchatui::diagnostics"
                })),
        )
        .try_init()
        .context("cannot initialize application tracing")?;
    Ok(Logging {
        _guard: guard,
        directory,
    })
}

fn writer(directory: &Path) -> Result<(tracing_appender::non_blocking::NonBlocking, WorkerGuard)> {
    std::fs::create_dir_all(directory).context("cannot create application log directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .context("cannot protect application log directory")?;
    }
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("gchatui")
        .filename_suffix("log")
        .max_log_files(7)
        .build(directory)
        .context("cannot open application log file")?;
    Ok(
        tracing_appender::non_blocking::NonBlockingBuilder::default()
            .lossy(false)
            .finish(appender),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_sink_flushes_sanitized_events_and_excludes_dependencies() {
        let directory = tempfile::tempdir().unwrap();
        let (writer, guard) = writer(directory.path()).unwrap();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_ansi(false)
                .with_writer(writer)
                .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                    metadata.target() == "gchatui::diagnostics"
                })),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "gchatui::diagnostics", image_id = 1, outcome = "decode_failed", "image_complete");
            tracing::warn!(target: "reqwest", "synthetic-private-marker");
        });
        drop(guard);
        let entry = std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let text = std::fs::read_to_string(entry.path()).unwrap();
        assert!(text.contains("image_complete"));
        assert!(!text.contains("synthetic-private-marker"));
        for line in text.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
    }
}
