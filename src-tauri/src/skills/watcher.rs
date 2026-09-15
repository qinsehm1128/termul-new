use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TrySendError};
use std::sync::Arc;

const QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsWatchEvent {
    Changed { paths: Vec<PathBuf> },
    Stale { reason: String },
    FullRescan,
}

pub struct SkillsWatcher {
    watcher: RecommendedWatcher,
    receiver: Receiver<SkillsWatchEvent>,
    backpressure: Arc<AtomicBool>,
}

impl SkillsWatcher {
    pub fn start(roots: &[PathBuf]) -> Result<Self, String> {
        let (sender, receiver) = sync_channel(QUEUE_CAPACITY);
        let callback_sender = sender.clone();
        let backpressure = Arc::new(AtomicBool::new(false));
        let callback_backpressure = Arc::clone(&backpressure);
        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let event = match result {
                Ok(event) => SkillsWatchEvent::Changed { paths: event.paths },
                Err(error) => SkillsWatchEvent::Stale {
                    reason: error.to_string(),
                },
            };
            match callback_sender.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    callback_backpressure.store(true, Ordering::Release);
                }
                Err(TrySendError::Disconnected(_)) => {}
            }
        })
        .map_err(|error| format!("create skills watcher: {error}"))?;
        let mut result = Self {
            watcher,
            receiver,
            backpressure,
        };
        for root in roots {
            if root.is_dir() {
                result
                    .watcher
                    .watch(root, RecursiveMode::Recursive)
                    .map_err(|error| format!("watch {}: {error}", root.display()))?;
            }
        }
        Ok(result)
    }

    pub fn try_next(&self) -> Option<SkillsWatchEvent> {
        match self.receiver.try_recv() {
            Ok(event) => Some(event),
            Err(std::sync::mpsc::TryRecvError::Empty)
                if self.backpressure.swap(false, Ordering::AcqRel) =>
            {
                Some(SkillsWatchEvent::Stale {
                    reason: "WATCHER_BACKPRESSURE".to_string(),
                })
            }
            Err(_) => None,
        }
    }
}

pub fn relevant_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .map(|relative| {
            !relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::Normal(name) if matches!(
                        name.to_str(),
                        Some("node_modules" | "vendor" | ".git" | "cache" | "build")
                    )
                )
            })
        })
        .unwrap_or(false)
}

#[allow(dead_code)]
fn _event_kind_is_content_change(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_ignored_segments_and_keeps_skill_paths() {
        let root = Path::new("/tmp/skills");
        assert!(relevant_path(root, Path::new("/tmp/skills/demo/SKILL.md")));
        assert!(!relevant_path(
            root,
            Path::new("/tmp/skills/node_modules/demo/SKILL.md")
        ));
        assert!(!relevant_path(
            root,
            Path::new("/tmp/skills/.git/demo/SKILL.md")
        ));
        assert!(!relevant_path(
            root,
            Path::new("/tmp/skills/cache/demo/SKILL.md")
        ));
    }

    #[test]
    fn start_and_drop_on_temp_root() {
        let temp = tempfile::tempdir().unwrap();
        let watcher = SkillsWatcher::start(&[temp.path().to_path_buf()]).unwrap();
        assert!(watcher.try_next().is_none());
        drop(watcher);
    }
}
