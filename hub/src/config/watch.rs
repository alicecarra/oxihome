use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use super::FILES;

/// One saved file makes several events (write, close, rename), so changes are reported once
/// the directory has been quiet this long.
pub const QUIET: Duration = Duration::from_secs(1);

/// Watches the directory, not the files: editors save by renaming a new file over the old one.
/// The receiver gets one `()` per burst of changes to `FILES`, `quiet` after the last one; the
/// watcher stops when dropped.
pub fn watch(
    directory: &Path,
    quiet: Duration,
) -> anyhow::Result<(RecommendedWatcher, mpsc::Receiver<()>)> {
    let (events_sender, mut events) = mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result
            && is_config_change(&event)
        {
            let _ = events_sender.send(());
        }
    })?;
    watcher
        .watch(directory, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", directory.display()))?;

    let (changes_sender, changes) = mpsc::channel(1);
    tokio::spawn(async move {
        while events.recv().await.is_some() {
            while let Ok(Some(())) = tokio::time::timeout(quiet, events.recv()).await {}
            if changes_sender.send(()).await.is_err() {
                return;
            }
        }
    });
    Ok((watcher, changes))
}

fn is_config_change(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) && event.paths.iter().any(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| FILES.contains(&name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::write_file;

    const TEST_QUIET: Duration = Duration::from_millis(200);

    fn directory(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("oxihome-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    async fn next_change(changes: &mut mpsc::Receiver<()>) -> bool {
        tokio::time::timeout(Duration::from_secs(5), changes.recv())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn burst_of_writes_is_one_change() {
        let directory = directory("burst");
        let (_watcher, mut changes) = watch(&directory, TEST_QUIET).unwrap();

        std::fs::write(directory.join("rules.toml"), "a").unwrap();
        write_file(&directory.join("rules.toml"), "b").unwrap();
        std::fs::write(directory.join("nodes.toml"), "c").unwrap();

        assert!(next_change(&mut changes).await);
        let again = tokio::time::timeout(TEST_QUIET * 3, changes.recv()).await;
        assert!(again.is_err(), "a second change was reported");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn other_files_are_ignored() {
        let directory = directory("other");
        let (_watcher, mut changes) = watch(&directory, TEST_QUIET).unwrap();

        std::fs::write(directory.join("notes.txt"), "a").unwrap();
        let change = tokio::time::timeout(TEST_QUIET * 3, changes.recv()).await;
        assert!(change.is_err(), "a change was reported");

        write_file(&directory.join("dashboard.toml"), "b").unwrap();
        assert!(next_change(&mut changes).await);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
