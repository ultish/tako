//! In-TUI workspace list editors: scan roots + project excludes (M1.5 / exclude).

use std::path::{Path, PathBuf};

use super::App;
use crate::config::{self, expand_user_path};
use crate::events::Command;
use crate::text_field;

/// Which workspace list the form is editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEditorKind {
    /// `[scan].roots` — absolute / `~/` path.
    Root,
    /// `[scan].exclude` — name, path fragment, or glob pattern.
    Exclude,
}

/// Mode for the text field overlay on the Workspace screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootEditorMode {
    /// Append a new entry.
    Add,
    /// Replace list entry at `index`.
    Edit(usize),
}

/// Text field state while adding or editing a root path or exclude pattern.
#[derive(Debug, Clone)]
pub struct RootEditorState {
    pub kind: WorkspaceEditorKind,
    pub mode: RootEditorMode,
    pub path: String,
    /// Cursor as a **char** index within `path` (0..=len).
    pub cursor: usize,
    pub error: Option<String>,
}

impl RootEditorState {
    pub fn new_add() -> Self {
        Self::new_add_kind(WorkspaceEditorKind::Root)
    }

    pub fn new_add_exclude() -> Self {
        Self::new_add_kind(WorkspaceEditorKind::Exclude)
    }

    fn new_add_kind(kind: WorkspaceEditorKind) -> Self {
        Self {
            kind,
            mode: RootEditorMode::Add,
            path: String::new(),
            cursor: 0,
            error: None,
        }
    }

    pub fn new_edit(index: usize, existing: &str) -> Self {
        Self::new_edit_kind(WorkspaceEditorKind::Root, index, existing)
    }

    pub fn new_edit_exclude(index: usize, existing: &str) -> Self {
        Self::new_edit_kind(WorkspaceEditorKind::Exclude, index, existing)
    }

    fn new_edit_kind(kind: WorkspaceEditorKind, index: usize, existing: &str) -> Self {
        let path = existing.to_string();
        let cursor = path.chars().count();
        Self {
            kind,
            mode: RootEditorMode::Edit(index),
            path,
            cursor,
            error: None,
        }
    }

    pub fn is_edit(&self) -> bool {
        matches!(self.mode, RootEditorMode::Edit(_))
    }

    pub fn is_exclude(&self) -> bool {
        matches!(self.kind, WorkspaceEditorKind::Exclude)
    }

    pub fn insert_char(&mut self, c: char) {
        text_field::insert_char(&mut self.path, &mut self.cursor, c);
        self.error = None;
    }

    pub fn backspace(&mut self) {
        text_field::backspace(&mut self.path, &mut self.cursor);
        self.error = None;
    }

    pub fn delete_forward(&mut self) {
        text_field::delete_forward(&mut self.path, &mut self.cursor);
        self.error = None;
    }

    pub fn cursor_left(&mut self) {
        text_field::cursor_left(&mut self.cursor);
    }

    pub fn cursor_right(&mut self) {
        text_field::cursor_right(&self.path, &mut self.cursor);
    }

    pub fn cursor_home(&mut self) {
        text_field::cursor_home(&mut self.cursor);
    }

    pub fn cursor_end(&mut self) {
        text_field::cursor_end(&self.path, &mut self.cursor);
    }

    pub fn display_with_cursor(&self) -> String {
        text_field::display_with_cursor(&self.path, self.cursor)
    }

    /// Validate field → string for config.
    pub fn validated_value(&self) -> Result<String, String> {
        match self.kind {
            WorkspaceEditorKind::Root => normalize_root_path(&self.path),
            WorkspaceEditorKind::Exclude => normalize_exclude_pattern(&self.path),
        }
    }

    /// Back-compat alias used by older call sites.
    pub fn validated_path(&self) -> Result<String, String> {
        self.validated_value()
    }
}

/// Expand `~/`, require non-empty and absolute (or home-relative) path.
pub fn normalize_root_path(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("path is required".into());
    }

    // Allow absolute paths and `~/…` / `~`. Relative paths (no leading `/` or `~`)
    // are rejected so scan roots are unambiguous across machines.
    let looks_home = trimmed == "~" || trimmed.starts_with("~/");
    let looks_abs = Path::new(trimmed).is_absolute();
    if !looks_home && !looks_abs {
        return Err("path must be absolute or start with ~/".into());
    }

    let expanded = expand_user_path(trimmed);
    if !expanded.is_absolute() {
        // `~` expand failed (no home dir) and left a non-absolute string.
        return Err("could not expand ~ — home directory unknown".into());
    }

    // Store a clean display form (no trailing slash except root `/`).
    Ok(path_to_config_string(&expanded))
}

/// Exclude patterns: non-empty free text (name, path fragment, or glob).
pub fn normalize_exclude_pattern(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("pattern is required".into());
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("pattern must be a single line".into());
    }
    Ok(trimmed.to_string())
}

fn path_to_config_string(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    if s.len() > 1 && s.ends_with('/') {
        s.trim_end_matches('/').to_string()
    } else {
        s
    }
}

impl App {
    pub(super) fn start_add_root(&mut self) {
        self.root_delete_confirm = None;
        self.exclude_delete_confirm = None;
        self.workspace_panel = WorkspacePanel::Roots;
        self.root_editor = Some(RootEditorState::new_add());
    }

    pub(super) fn start_edit_root(&mut self) {
        let idx = self.selected_root_index;
        let Some(existing) = self.config.scan.roots.get(idx).cloned() else {
            self.set_ephemeral_status("no root selected to edit");
            return;
        };
        self.root_delete_confirm = None;
        self.exclude_delete_confirm = None;
        self.workspace_panel = WorkspacePanel::Roots;
        self.root_editor = Some(RootEditorState::new_edit(idx, &existing));
    }

    pub(super) fn start_delete_root(&mut self) {
        if self.config.scan.roots.is_empty() {
            self.set_ephemeral_status("no roots to delete");
            return;
        }
        let idx = self.selected_root_index.min(self.config.scan.roots.len() - 1);
        self.selected_root_index = idx;
        self.root_editor = None;
        self.exclude_delete_confirm = None;
        self.root_delete_confirm = Some(idx);
    }

    pub(super) fn start_add_exclude(&mut self) {
        self.root_delete_confirm = None;
        self.exclude_delete_confirm = None;
        self.workspace_panel = WorkspacePanel::Excludes;
        self.root_editor = Some(RootEditorState::new_add_exclude());
    }

    pub(super) fn start_edit_exclude(&mut self) {
        let idx = self.selected_exclude_index;
        let Some(existing) = self.config.scan.exclude.get(idx).cloned() else {
            self.set_ephemeral_status("no exclude selected to edit");
            return;
        };
        self.root_delete_confirm = None;
        self.exclude_delete_confirm = None;
        self.workspace_panel = WorkspacePanel::Excludes;
        self.root_editor = Some(RootEditorState::new_edit_exclude(idx, &existing));
    }

    pub(super) fn start_delete_exclude(&mut self) {
        if self.config.scan.exclude.is_empty() {
            self.set_ephemeral_status("no excludes to delete");
            return;
        }
        let idx = self
            .selected_exclude_index
            .min(self.config.scan.exclude.len() - 1);
        self.selected_exclude_index = idx;
        self.root_editor = None;
        self.root_delete_confirm = None;
        self.exclude_delete_confirm = Some(idx);
    }

    pub(super) fn cancel_root_editor(&mut self) {
        self.root_editor = None;
    }

    pub(super) fn cancel_delete_root(&mut self) {
        self.root_delete_confirm = None;
    }

    pub(super) fn cancel_delete_exclude(&mut self) {
        self.exclude_delete_confirm = None;
    }

    /// Removes the root pending deletion and saves config; rescans if any remain.
    pub(super) fn confirm_delete_root(&mut self) -> Vec<Command> {
        let Some(index) = self.root_delete_confirm.take() else {
            return vec![];
        };
        let Some(removed) = self.config.scan.roots.get(index).cloned() else {
            return vec![];
        };

        let backup = self.config.scan.roots.clone();
        self.config.scan.roots.remove(index);
        self.selected_root_index = self
            .selected_root_index
            .min(self.config.scan.roots.len().saturating_sub(1));

        if let Err(err) = config::save(&self.config_path, &self.config) {
            self.config.scan.roots = backup;
            self.set_ephemeral_status(format!("failed to delete root: {err}"));
            return vec![];
        }

        self.set_ephemeral_status(format!("deleted root '{removed}'"));
        self.maybe_rescan_after_roots_change()
    }

    pub(super) fn confirm_delete_exclude(&mut self) -> Vec<Command> {
        let Some(index) = self.exclude_delete_confirm.take() else {
            return vec![];
        };
        let Some(removed) = self.config.scan.exclude.get(index).cloned() else {
            return vec![];
        };

        let backup = self.config.scan.exclude.clone();
        self.config.scan.exclude.remove(index);
        self.selected_exclude_index = self
            .selected_exclude_index
            .min(self.config.scan.exclude.len().saturating_sub(1));

        if let Err(err) = config::save(&self.config_path, &self.config) {
            self.config.scan.exclude = backup;
            self.set_ephemeral_status(format!("failed to delete exclude: {err}"));
            return vec![];
        }

        self.set_ephemeral_status(format!("deleted exclude '{removed}'"));
        self.maybe_rescan_after_roots_change()
    }

    /// Validate field, write `[scan].roots` or `[scan].exclude`, toast, optional rescan.
    pub(super) fn save_root(&mut self) -> Vec<Command> {
        let Some(state) = self.root_editor.as_ref() else {
            return vec![];
        };
        let kind = state.kind;
        let mode = state.mode;
        let value = match state.validated_value() {
            Ok(p) => p,
            Err(err) => {
                if let Some(s) = self.root_editor.as_mut() {
                    s.error = Some(err);
                }
                return vec![];
            }
        };

        match kind {
            WorkspaceEditorKind::Root => self.save_root_value(mode, value),
            WorkspaceEditorKind::Exclude => self.save_exclude_value(mode, value),
        }
    }

    fn save_root_value(&mut self, mode: RootEditorMode, path: String) -> Vec<Command> {
        let edit_idx = match mode {
            RootEditorMode::Add => None,
            RootEditorMode::Edit(i) => Some(i),
        };
        let dup = self.config.scan.roots.iter().enumerate().any(|(i, r)| {
            Some(i) != edit_idx && paths_equal(r, &path)
        });
        if dup {
            if let Some(s) = self.root_editor.as_mut() {
                s.error = Some(format!("root already listed: {path}"));
            }
            return vec![];
        }

        let backup = self.config.scan.roots.clone();
        let verb;
        match mode {
            RootEditorMode::Add => {
                self.config.scan.roots.push(path.clone());
                self.selected_root_index = self.config.scan.roots.len() - 1;
                verb = "added";
            }
            RootEditorMode::Edit(idx) => {
                if idx >= self.config.scan.roots.len() {
                    if let Some(s) = self.root_editor.as_mut() {
                        s.error = Some("root no longer exists".into());
                    }
                    return vec![];
                }
                self.config.scan.roots[idx] = path.clone();
                self.selected_root_index = idx;
                verb = "updated";
            }
        }

        if let Err(err) = config::save(&self.config_path, &self.config) {
            self.config.scan.roots = backup;
            if let Some(s) = self.root_editor.as_mut() {
                s.error = Some(format!("failed to save config: {err}"));
            }
            return vec![];
        }

        self.root_editor = None;
        self.workspace_panel = WorkspacePanel::Roots;
        self.set_ephemeral_status(format!("{verb} root '{path}'"));
        self.maybe_rescan_after_roots_change()
    }

    fn save_exclude_value(&mut self, mode: RootEditorMode, pattern: String) -> Vec<Command> {
        let edit_idx = match mode {
            RootEditorMode::Add => None,
            RootEditorMode::Edit(i) => Some(i),
        };
        let dup = self
            .config
            .scan
            .exclude
            .iter()
            .enumerate()
            .any(|(i, r)| Some(i) != edit_idx && r == &pattern);
        if dup {
            if let Some(s) = self.root_editor.as_mut() {
                s.error = Some(format!("exclude already listed: {pattern}"));
            }
            return vec![];
        }

        let backup = self.config.scan.exclude.clone();
        let verb;
        match mode {
            RootEditorMode::Add => {
                self.config.scan.exclude.push(pattern.clone());
                self.selected_exclude_index = self.config.scan.exclude.len() - 1;
                verb = "added";
            }
            RootEditorMode::Edit(idx) => {
                if idx >= self.config.scan.exclude.len() {
                    if let Some(s) = self.root_editor.as_mut() {
                        s.error = Some("exclude no longer exists".into());
                    }
                    return vec![];
                }
                self.config.scan.exclude[idx] = pattern.clone();
                self.selected_exclude_index = idx;
                verb = "updated";
            }
        }

        if let Err(err) = config::save(&self.config_path, &self.config) {
            self.config.scan.exclude = backup;
            if let Some(s) = self.root_editor.as_mut() {
                s.error = Some(format!("failed to save config: {err}"));
            }
            return vec![];
        }

        self.root_editor = None;
        self.workspace_panel = WorkspacePanel::Excludes;
        self.set_ephemeral_status(format!("{verb} exclude '{pattern}'"));
        self.maybe_rescan_after_roots_change()
    }

    /// After roots/excludes change: rescan when non-empty; clear inventory when empty.
    ///
    /// Does **not** set the sticky "scanning…" status here — that happens when
    /// main handles `Command::ScanWorkspace` / `AppEvent::ScanStarted`, so the
    /// save/delete toast can show first.
    fn maybe_rescan_after_roots_change(&mut self) -> Vec<Command> {
        if self.config.scan.roots.is_empty() {
            self.projects.clear();
            self.selected_index = 0;
            self.scanning = false;
            return vec![];
        }
        vec![Command::ScanWorkspace]
    }

    pub(super) fn move_root_selection(&mut self, delta: i32) {
        let len = self.config.scan.roots.len();
        if len == 0 {
            return;
        }
        let cur = self.selected_root_index as i32;
        let next = (cur + delta).clamp(0, (len as i32) - 1) as usize;
        self.selected_root_index = next;
    }

    pub(super) fn move_exclude_selection(&mut self, delta: i32) {
        let len = self.config.scan.exclude.len();
        if len == 0 {
            return;
        }
        let cur = self.selected_exclude_index as i32;
        let next = (cur + delta).clamp(0, (len as i32) - 1) as usize;
        self.selected_exclude_index = next;
    }

    pub(super) fn clamp_root_selection(&mut self) {
        if self.config.scan.roots.is_empty() {
            self.selected_root_index = 0;
        } else if self.selected_root_index >= self.config.scan.roots.len() {
            self.selected_root_index = self.config.scan.roots.len() - 1;
        }
    }

    pub(super) fn clamp_exclude_selection(&mut self) {
        if self.config.scan.exclude.is_empty() {
            self.selected_exclude_index = 0;
        } else if self.selected_exclude_index >= self.config.scan.exclude.len() {
            self.selected_exclude_index = self.config.scan.exclude.len() - 1;
        }
    }

    pub(super) fn toggle_workspace_panel(&mut self) {
        self.workspace_panel = match self.workspace_panel {
            WorkspacePanel::Roots => WorkspacePanel::Excludes,
            WorkspacePanel::Excludes => WorkspacePanel::Roots,
        };
        self.clamp_root_selection();
        self.clamp_exclude_selection();
    }
}

/// Focused list on the Workspace screen (Tab toggles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkspacePanel {
    #[default]
    Roots,
    Excludes,
}

fn paths_equal(a: &str, b: &str) -> bool {
    let pa = expand_user_path(a.trim());
    let pb = PathBuf::from(b);
    pa == pb || a.trim() == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_root_path("").is_err());
        assert!(normalize_root_path("   ").is_err());
        assert!(normalize_exclude_pattern("").is_err());
    }

    #[test]
    fn normalize_rejects_relative() {
        assert!(normalize_root_path("Developer/foo").is_err());
        assert!(normalize_root_path("./here").is_err());
    }

    #[test]
    fn normalize_accepts_absolute() {
        let p = normalize_root_path("/tmp/workspace").expect("ok");
        assert_eq!(p, "/tmp/workspace");
    }

    #[test]
    fn normalize_expands_tilde() {
        if dirs::home_dir().is_none() {
            return;
        }
        let p = normalize_root_path("~/Developer").expect("ok");
        assert!(p.ends_with("Developer") || p.contains("Developer"));
        assert!(Path::new(&p).is_absolute());
    }

    #[test]
    fn normalize_exclude_keeps_patterns() {
        assert_eq!(
            normalize_exclude_pattern("  legacy-api  ").unwrap(),
            "legacy-api"
        );
        assert_eq!(
            normalize_exclude_pattern("**/sandbox/**").unwrap(),
            "**/sandbox/**"
        );
    }
}
