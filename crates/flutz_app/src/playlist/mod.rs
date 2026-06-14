use std::path::PathBuf;

pub mod persistence;

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum PlaylistRepeatMode {
    #[default]
    Off,
    Track,
    Playlist,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum PlaylistOrderMode {
    #[default]
    Sequential,
    Shuffle,
    Random,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum PlaylistEntryStatus {
    #[default]
    Valid,
    Missing,
    CurrentlyPlaying,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistEntry {
    pub file_path: PathBuf,
    pub display_name: String,
    pub file_exists: bool,
    pub status: PlaylistEntryStatus,
}

impl PlaylistEntry {
    pub fn from_path(file_path: PathBuf) -> Self {
        let display_name = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| file_path.display().to_string());
        let mut entry = Self {
            file_path,
            display_name,
            file_exists: false,
            status: PlaylistEntryStatus::Missing,
        };
        entry.refresh_status();
        entry
    }

    pub fn refresh_status(&mut self) {
        self.file_exists = self.file_path.exists();
        if self.status != PlaylistEntryStatus::CurrentlyPlaying {
            self.status = if self.file_exists {
                PlaylistEntryStatus::Valid
            } else {
                PlaylistEntryStatus::Missing
            };
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaylistState {
    pub entries: Vec<PlaylistEntry>,
    pub current_index: Option<usize>,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    pub loop_enabled: bool,
    pub repeat_mode: PlaylistRepeatMode,
    pub order_mode: PlaylistOrderMode,
    pub shuffle_seed: u64,
    pub history: Vec<usize>,
    shuffled_order: Vec<usize>,
    shuffled_cursor: usize,
    random_counter: u64,
}

impl PlaylistState {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn add_entry(&mut self, file_path: PathBuf) {
        self.entries.push(PlaylistEntry::from_path(file_path));
        if self.current_index.is_none() {
            self.set_current_index(Some(0));
        }
        self.rebuild_shuffle_order();
        self.dirty = true;
    }

    pub fn add_entries<I>(&mut self, file_paths: I)
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let start_was_empty = self.entries.is_empty();
        for file_path in file_paths {
            self.entries.push(PlaylistEntry::from_path(file_path));
        }
        if start_was_empty && !self.entries.is_empty() {
            self.set_current_index(Some(0));
        }
        if !self.entries.is_empty() {
            self.rebuild_shuffle_order();
            self.dirty = true;
        }
    }

    pub fn prepend_entry(&mut self, file_path: PathBuf) {
        if let Some(existing_index) = self
            .entries
            .iter()
            .position(|entry| entry.file_path == file_path)
        {
            self.entries.remove(existing_index);
        }

        self.entries.insert(0, PlaylistEntry::from_path(file_path));
        self.history.clear();
        self.set_current_index(Some(0));
        self.rebuild_shuffle_order();
        self.dirty = true;
    }

    pub fn remove_indices(&mut self, indices: &[usize]) {
        if indices.is_empty() || self.entries.is_empty() {
            return;
        }

        let mut sorted = indices.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.retain(|index| *index < self.entries.len());
        if sorted.is_empty() {
            return;
        }

        let original_current = self.current_index;
        let mut removed_before_current = 0usize;
        let mut removed_current = false;
        if let Some(current) = original_current {
            for index in &sorted {
                if *index < current {
                    removed_before_current += 1;
                } else if *index == current {
                    removed_current = true;
                }
            }
        }

        for index in sorted.iter().rev() {
            self.entries.remove(*index);
        }

        if self.entries.is_empty() {
            self.set_current_index(None);
        } else if let Some(current) = original_current {
            let replacement = current.saturating_sub(removed_before_current);
            let _ = removed_current;
            self.set_current_index(Some(replacement.min(self.entries.len() - 1)));
        }

        self.rebuild_shuffle_order();
        self.dirty = true;
    }

    pub fn move_entry(&mut self, from_index: usize, to_index: usize) {
        if from_index >= self.entries.len() || to_index >= self.entries.len() || from_index == to_index {
            return;
        }

        let entry = self.entries.remove(from_index);
        self.entries.insert(to_index, entry);

        if let Some(current) = self.current_index {
            let remapped = if current == from_index {
                to_index
            } else if from_index < current && current <= to_index {
                current - 1
            } else if to_index <= current && current < from_index {
                current + 1
            } else {
                current
            };
            self.set_current_index(Some(remapped));
        }

        self.rebuild_shuffle_order();
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        if self.entries.is_empty() && self.current_index.is_none() {
            return;
        }
        self.entries.clear();
        self.current_index = None;
        self.history.clear();
        self.shuffled_order.clear();
        self.shuffled_cursor = 0;
        self.dirty = true;
    }

    pub fn set_repeat_mode(&mut self, mode: PlaylistRepeatMode) {
        if self.repeat_mode != mode {
            self.repeat_mode = mode;
            self.dirty = true;
        }
    }

    pub fn set_order_mode(&mut self, mode: PlaylistOrderMode) {
        if self.order_mode != mode {
            self.order_mode = mode;
            self.rebuild_shuffle_order();
            self.dirty = true;
        }
    }

    pub fn set_current_index(&mut self, current_index: Option<usize>) {
        for entry in &mut self.entries {
            if entry.status == PlaylistEntryStatus::CurrentlyPlaying {
                entry.status = if entry.file_exists {
                    PlaylistEntryStatus::Valid
                } else {
                    PlaylistEntryStatus::Missing
                };
            }
        }

        self.current_index = current_index.filter(|index| *index < self.entries.len());

        if let Some(index) = self.current_index {
            if let Some(entry) = self.entries.get_mut(index) {
                entry.refresh_status();
                entry.status = PlaylistEntryStatus::CurrentlyPlaying;
            }
        }
    }

    pub fn set_current_with_history(&mut self, next_index: usize, push_history: bool) {
        if push_history {
            if let Some(current) = self.current_index {
                self.history.push(current);
                if self.history.len() > 4096 {
                    let truncate = self.history.len().saturating_sub(4096);
                    self.history.drain(0..truncate);
                }
            }
        }
        self.set_current_index(Some(next_index));
    }

    pub fn pop_previous_history(&mut self) -> Option<usize> {
        while let Some(index) = self.history.pop() {
            if index < self.entries.len() {
                return Some(index);
            }
        }
        None
    }

    pub fn next_valid_track(&mut self, wrap: bool) -> Option<usize> {
        self.next_valid_track_by(wrap, |_| true)
    }

    pub fn prev_valid_track(&mut self, wrap: bool) -> Option<usize> {
        self.prev_valid_track_by(wrap, |_| true)
    }

    pub fn first_valid_track_by<F>(&mut self, mut is_valid: F) -> Option<usize>
    where
        F: FnMut(&PlaylistEntry) -> bool,
    {
        for index in 0..self.entries.len() {
            let entry = &mut self.entries[index];
            entry.refresh_status();
            if entry.file_exists && is_valid(entry) {
                return Some(index);
            }
        }
        None
    }

    pub fn next_track_for_mode_by<F>(&mut self, is_valid: F) -> Option<usize>
    where
        F: FnMut(&PlaylistEntry) -> bool,
    {
        match self.order_mode {
            PlaylistOrderMode::Sequential => self.next_valid_track_by(
                self.repeat_mode == PlaylistRepeatMode::Playlist,
                is_valid,
            ),
            PlaylistOrderMode::Shuffle => self.next_shuffle_track_by(is_valid),
            PlaylistOrderMode::Random => self.next_random_track_by(is_valid),
        }
    }

    pub fn prev_track_for_mode_by<F>(&mut self, mut is_valid: F) -> Option<usize>
    where
        F: FnMut(&PlaylistEntry) -> bool,
    {
        if matches!(self.order_mode, PlaylistOrderMode::Shuffle | PlaylistOrderMode::Random) {
            return self.pop_previous_history().filter(|index| {
                let entry = &mut self.entries[*index];
                entry.refresh_status();
                entry.file_exists && is_valid(entry)
            });
        }

        self.prev_valid_track_by(self.repeat_mode == PlaylistRepeatMode::Playlist, is_valid)
    }

    pub fn next_valid_track_by<F>(&mut self, wrap: bool, mut is_valid: F) -> Option<usize>
    where
        F: FnMut(&PlaylistEntry) -> bool,
    {
        let current = self.current_index?;
        if self.entries.is_empty() {
            return None;
        }

        for index in (current + 1)..self.entries.len() {
            let entry = &mut self.entries[index];
            entry.refresh_status();
            if entry.file_exists && is_valid(entry) {
                return Some(index);
            }
        }

        if wrap {
            for index in 0..current {
                let entry = &mut self.entries[index];
                entry.refresh_status();
                if entry.file_exists && is_valid(entry) {
                    return Some(index);
                }
            }
        }

        None
    }

    pub fn prev_valid_track_by<F>(&mut self, wrap: bool, mut is_valid: F) -> Option<usize>
    where
        F: FnMut(&PlaylistEntry) -> bool,
    {
        let current = self.current_index?;
        if self.entries.is_empty() {
            return None;
        }

        for index in (0..current).rev() {
            let entry = &mut self.entries[index];
            entry.refresh_status();
            if entry.file_exists && is_valid(entry) {
                return Some(index);
            }
        }

        if wrap {
            for index in ((current + 1)..self.entries.len()).rev() {
                let entry = &mut self.entries[index];
                entry.refresh_status();
                if entry.file_exists && is_valid(entry) {
                    return Some(index);
                }
            }
        }

        None
    }

    fn rebuild_shuffle_order(&mut self) {
        self.shuffled_order = (0..self.entries.len()).collect::<Vec<_>>();
        if self.shuffled_order.len() > 1 {
            let mut state = self.shuffle_seed ^ self.entries.len() as u64;
            for i in (1..self.shuffled_order.len()).rev() {
                state = lcg_next(state);
                let swap_index = (state as usize) % (i + 1);
                self.shuffled_order.swap(i, swap_index);
            }
        }
        self.shuffled_cursor = 0;
    }

    fn next_shuffle_track_by<F>(&mut self, mut is_valid: F) -> Option<usize>
    where
        F: FnMut(&PlaylistEntry) -> bool,
    {
        if self.shuffled_order.len() != self.entries.len() {
            self.rebuild_shuffle_order();
        }
        if self.shuffled_order.is_empty() {
            return None;
        }

        for _ in 0..(self.shuffled_order.len() * 2).max(1) {
            if self.shuffled_cursor >= self.shuffled_order.len() {
                self.rebuild_shuffle_order();
            }
            let index = self.shuffled_order[self.shuffled_cursor];
            self.shuffled_cursor += 1;

            if Some(index) == self.current_index && self.entries.len() > 1 {
                continue;
            }

            let entry = &mut self.entries[index];
            entry.refresh_status();
            if entry.file_exists && is_valid(entry) {
                return Some(index);
            }
        }

        None
    }

    fn next_random_track_by<F>(&mut self, mut is_valid: F) -> Option<usize>
    where
        F: FnMut(&PlaylistEntry) -> bool,
    {
        if self.entries.is_empty() {
            return None;
        }

        let mut candidate_indices = Vec::new();
        for index in 0..self.entries.len() {
            let entry = &mut self.entries[index];
            entry.refresh_status();
            if entry.file_exists && is_valid(entry) {
                candidate_indices.push(index);
            }
        }

        if candidate_indices.is_empty() {
            return None;
        }

        if candidate_indices.len() > 1 {
            candidate_indices.retain(|index| Some(*index) != self.current_index);
            if candidate_indices.is_empty() {
                return self.current_index;
            }
        }

        self.random_counter = lcg_next(self.random_counter ^ self.shuffle_seed);
        let pick = (self.random_counter as usize) % candidate_indices.len();
        candidate_indices.get(pick).copied()
    }
}

fn lcg_next(state: u64) -> u64 {
    state.wrapping_mul(6364136223846793005).wrapping_add(1)
}
