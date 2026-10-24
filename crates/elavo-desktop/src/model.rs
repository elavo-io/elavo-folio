//! Grid, selection, stacks, and which mounts belong on the desktop.
//! No GTK types, so the rules can be tested without a display.

use std::collections::{BTreeMap, HashSet};

use gtk::glib;

pub const CELL_W: i32 = 108;
pub const CELL_H: i32 = 96;
pub const MARGIN_X: i32 = 16;
pub const MARGIN_TOP: i32 = 40;
pub const MARGIN_BOTTOM: i32 = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Folder,
    Image,
    Document,
    Pdf,
    Music,
    Video,
    Archive,
    Application,
    Other,
}

impl Kind {
    pub fn id(self) -> &'static str {
        match self {
            Kind::Folder => "folder",
            Kind::Image => "image",
            Kind::Document => "document",
            Kind::Pdf => "pdf",
            Kind::Music => "music",
            Kind::Video => "video",
            Kind::Archive => "archive",
            Kind::Application => "application",
            Kind::Other => "other",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        [
            Kind::Folder,
            Kind::Image,
            Kind::Document,
            Kind::Pdf,
            Kind::Music,
            Kind::Video,
            Kind::Archive,
            Kind::Application,
            Kind::Other,
        ]
        .into_iter()
        .find(|kind| kind.id() == id)
    }

    /// The stack's label. Volumes are never stacked.
    pub fn group_name(self) -> &'static str {
        match self {
            Kind::Folder => "Folders",
            Kind::Image => "Images",
            Kind::Document => "Documents",
            Kind::Pdf => "PDFs",
            Kind::Music => "Music",
            Kind::Video => "Videos",
            Kind::Archive => "Archives",
            Kind::Application => "Applications",
            Kind::Other => "Other",
        }
    }

    /// A content type the icon theme can draw for a stack of this kind.
    pub fn mime(self) -> &'static str {
        match self {
            Kind::Folder => "inode/directory",
            Kind::Image => "image/png",
            Kind::Document => "text/plain",
            Kind::Pdf => "application/pdf",
            Kind::Music => "audio/mpeg",
            Kind::Video => "video/mp4",
            Kind::Archive => "application/zip",
            Kind::Application => "application/x-desktop",
            Kind::Other => "application/octet-stream",
        }
    }

    pub fn from_content_type(mime: &str, is_dir: bool) -> Self {
        if is_dir || mime == "inode/directory" {
            return Kind::Folder;
        }
        if mime == "application/pdf" || mime == "application/x-pdf" {
            return Kind::Pdf;
        }
        if mime.starts_with("image/") {
            return Kind::Image;
        }
        if mime.starts_with("audio/") {
            return Kind::Music;
        }
        if mime.starts_with("video/") {
            return Kind::Video;
        }
        if mime == "application/x-desktop" || mime == "application/vnd.elavo.application" {
            return Kind::Application;
        }
        if is_archive(mime) {
            return Kind::Archive;
        }
        if is_document(mime) {
            return Kind::Document;
        }
        Kind::Other
    }
}

fn is_archive(mime: &str) -> bool {
    matches!(
        mime,
        "application/zip"
            | "application/gzip"
            | "application/x-gzip"
            | "application/x-tar"
            | "application/x-bzip"
            | "application/x-bzip2"
            | "application/x-xz"
            | "application/zstd"
            | "application/x-7z-compressed"
            | "application/vnd.rar"
            | "application/x-rar"
            | "application/java-archive"
            | "application/x-compressed-tar"
            | "application/x-bzip-compressed-tar"
            | "application/x-xz-compressed-tar"
    )
}

fn is_document(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime.starts_with("application/vnd.oasis.opendocument")
        || mime.starts_with("application/vnd.openxmlformats-officedocument")
        || mime.starts_with("application/vnd.ms-")
        || matches!(
            mime,
            "application/rtf"
                | "application/json"
                | "application/xml"
                | "application/xhtml+xml"
                | "application/epub+zip"
                | "application/msword"
        )
}

/// Facts the volume monitor can answer. The desktop shows removable disks,
/// optical discs, network mounts, and anything under the usual media paths.
/// The root filesystem and shadowed mounts stay off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountFacts {
    pub shadowed: bool,
    pub root: String,
    pub removable: bool,
    pub optical: bool,
    pub network: bool,
}

pub fn show_mount(facts: &MountFacts) -> bool {
    if facts.shadowed || facts.root == "/" {
        return false;
    }
    if facts.removable || facts.optical || facts.network {
        return true;
    }
    facts.root.starts_with("/run/media/")
        || facts.root.starts_with("/media/")
        || facts.root.starts_with("/mnt/")
}

/// What Use Stacks groups by, as in Finder's Group Stacks By. Date Added and
/// Tags are left out: Linux keeps no date a file was added to a folder, and
/// files carry no tags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GroupBy {
    #[default]
    Kind,
    Opened,
    Modified,
    Created,
}

impl GroupBy {
    pub const ALL: [GroupBy; 4] = [
        GroupBy::Kind,
        GroupBy::Opened,
        GroupBy::Modified,
        GroupBy::Created,
    ];

    pub fn id(self) -> &'static str {
        match self {
            GroupBy::Kind => "kind",
            GroupBy::Opened => "opened",
            GroupBy::Modified => "modified",
            GroupBy::Created => "created",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GroupBy::Kind => "Kind",
            GroupBy::Opened => "Date Last Opened",
            GroupBy::Modified => "Date Modified",
            GroupBy::Created => "Date Created",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|group| group.id() == id)
    }
}

/// Seconds since the epoch. 0 when the file system does not say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Times {
    pub opened: i64,
    pub modified: i64,
    pub created: i64,
}

/// One stack: its id in the layout file, its label, and its place in order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    id: String,
    label: String,
    rank: i64,
}

/// Finder's date stacks: Today, Yesterday, Previous 7 Days, Previous 30
/// Days, then each earlier month of this year, then each earlier year.
fn date_group(time: i64, now: i64) -> Group {
    let group = |id: &str, label: &str, rank: i64| Group {
        id: format!("date:{id}"),
        label: label.to_string(),
        rank,
    };
    if time <= 0 {
        return group("unknown", "Unknown", i64::MAX);
    }
    let (Ok(at), Ok(now_at)) = (
        glib::DateTime::from_unix_local(time),
        glib::DateTime::from_unix_local(now),
    ) else {
        return group("unknown", "Unknown", i64::MAX);
    };
    let Ok(today) = glib::DateTime::from_local(
        now_at.year(),
        now_at.month(),
        now_at.day_of_month(),
        0,
        0,
        0.0,
    ) else {
        return group("unknown", "Unknown", i64::MAX);
    };
    let days_back = |days: i32| {
        today
            .add_days(-days)
            .map(|day| day.to_unix())
            .unwrap_or(i64::MIN)
    };
    if time >= today.to_unix() {
        return group("today", "Today", 0);
    }
    if time >= days_back(1) {
        return group("yesterday", "Yesterday", 1);
    }
    if time >= days_back(7) {
        return group("week", "Previous 7 Days", 2);
    }
    if time >= days_back(30) {
        return group("month", "Previous 30 Days", 3);
    }
    if at.year() == now_at.year() {
        let month = at.month();
        let label = at
            .format("%B")
            .map(|text| text.to_string())
            .unwrap_or_else(|_| month.to_string());
        return group(&format!("m{month:02}"), &label, 100 + i64::from(12 - month));
    }
    let year = at.year();
    group(
        &format!("y{year}"),
        &year.to_string(),
        10_000 + i64::from(now_at.year() - year),
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Slot {
    pub col: i32,
    pub row: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Id {
    File(String),
    Mount(String),
    /// A stack by its group id: a kind (`image`) or a date (`date:today`).
    Stack(String),
}

#[derive(Clone, Debug)]
pub struct FileRec {
    pub name: String,
    pub mime: String,
    pub kind: Kind,
    pub times: Times,
}

#[derive(Clone, Debug)]
pub struct MountRec {
    pub uri: String,
    pub name: String,
    pub path: String,
    pub eject: bool,
    pub unmount: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: Id,
    pub slot: Slot,
    pub label: String,
    pub count: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub stacks: bool,
    pub group_by: GroupBy,
    slots: BTreeMap<String, Slot>,
}

impl Layout {
    pub fn parse(text: &str) -> Self {
        let mut layout = Layout::default();
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("stacks=") {
                layout.stacks = matches!(value.trim(), "true" | "1" | "yes");
                continue;
            }
            if let Some(value) = line.strip_prefix("group=") {
                layout.group_by = GroupBy::parse(value.trim()).unwrap_or_default();
                continue;
            }
            let Some((pos, key)) = line.split_once('\t') else {
                continue;
            };
            let Some((col, row)) = pos.split_once(',') else {
                continue;
            };
            let (Ok(col), Ok(row)) = (col.parse::<i32>(), row.parse::<i32>()) else {
                continue;
            };
            if key.is_empty() || key.contains('\t') {
                continue;
            }
            layout.slots.insert(
                key.to_string(),
                Slot {
                    col: col.max(0),
                    row: row.max(0),
                },
            );
        }
        layout
    }

    pub fn to_text(&self) -> String {
        let mut text = format!(
            "stacks={}\ngroup={}\n",
            if self.stacks { "true" } else { "false" },
            self.group_by.id()
        );
        for (key, slot) in &self.slots {
            text.push_str(&format!(
                "{col},{row}\t{key}\n",
                col = slot.col,
                row = slot.row
            ));
        }
        text
    }
}

pub struct Model {
    files: Vec<FileRec>,
    mounts: Vec<MountRec>,
    layout: Layout,
    selected: Vec<Id>,
    anchor: Option<Id>,
    grid: Grid,
    spread: Option<String>,
    fan: Vec<(String, Slot)>,
    /// The time date stacks are measured from.
    now: i64,
}

impl Model {
    pub fn new(cols: i32) -> Self {
        Self {
            files: Vec::new(),
            mounts: Vec::new(),
            layout: Layout::default(),
            selected: Vec::new(),
            anchor: None,
            grid: Grid::with_cols(cols),
            spread: None,
            fan: Vec::new(),
            now: 0,
        }
    }

    pub fn set_now(&mut self, now: i64) {
        self.now = now;
    }

    pub fn group_by(&self) -> GroupBy {
        self.layout.group_by
    }

    /// Stacks keep their places per grouping, and an open stack closes.
    pub fn set_group_by(&mut self, group_by: GroupBy) -> bool {
        if self.layout.group_by == group_by {
            return false;
        }
        self.layout.group_by = group_by;
        self.spread = None;
        self.fan.clear();
        self.prune();
        true
    }

    fn group_of(&self, file: &FileRec) -> Group {
        let time = match self.layout.group_by {
            GroupBy::Kind => {
                return Group {
                    id: file.kind.id().to_string(),
                    label: file.kind.group_name().to_string(),
                    rank: file.kind as i64,
                };
            }
            GroupBy::Opened => file.times.opened,
            GroupBy::Modified => file.times.modified,
            GroupBy::Created => file.times.created,
        };
        date_group(time, self.now)
    }

    fn in_group(&self, file: &FileRec, id: &str) -> bool {
        self.group_of(file).id == id
    }

    /// A content type the icon theme draws for the stack: the kind's for a
    /// kind stack, the newest member's for a date stack.
    pub fn stack_mime(&self, id: &str) -> String {
        if let Some(kind) = Kind::from_id(id) {
            return kind.mime().to_string();
        }
        self.files
            .iter()
            .filter(|file| self.in_group(file, id))
            .max_by_key(|file| file.times.modified)
            .map(|file| file.mime.clone())
            .unwrap_or_else(|| "inode/directory".to_string())
    }

    pub fn is_folder(&self, name: &str) -> bool {
        self.file(name)
            .is_some_and(|file| file.kind == Kind::Folder)
    }

    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
        self.spread = None;
        self.fan.clear();
        self.prune();
    }

    pub fn stacks(&self) -> bool {
        self.layout.stacks
    }

    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// Fits the grid to the desktop's size. True when cells moved.
    pub fn set_area(&mut self, width: i32, height: i32) -> bool {
        let grid = Grid::for_area(width, height);
        if grid == self.grid {
            return false;
        }
        self.grid = grid;
        if self.spread.is_some() {
            self.recompute_fan();
        }
        true
    }

    pub fn set_files(&mut self, files: Vec<FileRec>) {
        self.files = files;
        self.assign_file_slots();
        if self.spread.is_some() {
            self.recompute_fan();
        }
        self.prune();
    }

    pub fn set_mounts(&mut self, mounts: Vec<MountRec>) {
        self.mounts = mounts;
        self.assign_mount_slots();
        self.prune();
    }

    pub fn file_names(&self) -> Vec<String> {
        self.files.iter().map(|file| file.name.clone()).collect()
    }

    pub fn file(&self, name: &str) -> Option<&FileRec> {
        self.files.iter().find(|file| file.name == name)
    }

    pub fn mount(&self, uri: &str) -> Option<&MountRec> {
        self.mounts.iter().find(|mount| mount.uri == uri)
    }

    pub fn selection(&self) -> &[Id] {
        &self.selected
    }

    pub fn display_items(&self) -> Vec<Item> {
        let counts = self.group_counts();
        let mut items = Vec::new();
        if self.layout.stacks {
            let mut groups: Vec<&(Group, usize)> =
                counts.values().filter(|(_, count)| *count >= 2).collect();
            groups.sort_by_key(|(group, _)| group.rank);
            for (group, count) in groups {
                items.push(Item {
                    id: Id::Stack(group.id.clone()),
                    slot: self.stack_slot(&group.id),
                    label: group.label.clone(),
                    count: Some(*count as u32),
                });
            }
        }
        for file in &self.files {
            let group = self.group_of(file).id;
            let stacked = self.layout.stacks && counts.get(&group).is_some_and(|(_, n)| *n >= 2);
            let spread = self.spread.as_deref() == Some(group.as_str());
            if stacked && !spread {
                continue;
            }
            let slot = if spread {
                self.fan
                    .iter()
                    .find(|(name, _)| name == &file.name)
                    .map(|(_, slot)| *slot)
                    .unwrap_or_else(|| self.file_slot(&file.name))
            } else {
                self.file_slot(&file.name)
            };
            items.push(Item {
                id: Id::File(file.name.clone()),
                slot,
                label: file.name.clone(),
                count: None,
            });
        }
        for mount in &self.mounts {
            items.push(Item {
                id: Id::Mount(mount.uri.clone()),
                slot: self.mount_slot(&mount.uri),
                label: mount.name.clone(),
                count: None,
            });
        }
        items
    }

    pub fn plain_click(&mut self, id: &Id) {
        self.selected = vec![id.clone()];
        self.anchor = Some(id.clone());
    }

    pub fn ctrl_click(&mut self, id: &Id) {
        if let Some(index) = self.selected.iter().position(|item| item == id) {
            self.selected.remove(index);
            if self.anchor.as_ref() == Some(id) {
                self.anchor = self.selected.last().cloned();
            }
        } else {
            self.selected.push(id.clone());
            self.anchor = Some(id.clone());
        }
    }

    pub fn shift_click(&mut self, id: &Id) {
        let order = grid_order(&self.display_items(), self.grid);
        let Some(anchor) = self
            .anchor
            .clone()
            .or_else(|| self.selected.first().cloned())
        else {
            self.plain_click(id);
            return;
        };
        let (Some(start), Some(end)) = (
            order.iter().position(|item| item == &anchor),
            order.iter().position(|item| item == id),
        ) else {
            self.plain_click(id);
            return;
        };
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        self.selected = order[lo..=hi].to_vec();
    }

    pub fn select_ids(&mut self, ids: Vec<Id>) {
        self.selected = ids;
        self.anchor = self.selected.first().cloned();
    }

    pub fn select_all(&mut self) {
        self.selected = grid_order(&self.display_items(), self.grid);
        self.anchor = self.selected.first().cloned();
    }

    pub fn click_background(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.spread = None;
        self.fan.clear();
    }

    pub fn arrow(&mut self, dir: Dir) {
        let items = self.display_items();
        if items.is_empty() {
            return;
        }
        let current = self
            .anchor
            .clone()
            .or_else(|| self.selected.last().cloned());
        let Some(current) = current else {
            if let Some(first) = items
                .iter()
                .min_by_key(|item| (item.slot.row, item.slot.col))
            {
                self.plain_click(&first.id);
            }
            return;
        };
        let Some(from) = items.iter().find(|item| item.id == current) else {
            if let Some(first) = items
                .iter()
                .min_by_key(|item| (item.slot.row, item.slot.col))
            {
                self.plain_click(&first.id);
            }
            return;
        };
        if let Some(next) = neighbor(&items, from.slot, &from.id, dir) {
            self.plain_click(&next);
        }
    }

    pub fn toggle_stacks(&mut self) -> bool {
        self.layout.stacks = !self.layout.stacks;
        self.spread = None;
        self.fan.clear();
        self.prune();
        self.layout.stacks
    }

    pub fn toggle_spread(&mut self, id: &str) {
        if !self.layout.stacks || self.group_count(id) < 2 {
            return;
        }
        if self.spread.as_deref() == Some(id) {
            self.spread = None;
            self.fan.clear();
        } else {
            self.spread = Some(id.to_string());
            self.recompute_fan();
        }
        self.prune();
    }

    pub fn spread(&self) -> Option<&str> {
        self.spread.as_deref()
    }

    /// Move the grabbed icon to `to`. The rest of the selection keeps its
    /// offset from that icon. A cell already held by something else is skipped
    /// for the icon that would land on it.
    pub fn move_selection(&mut self, grabbed: &Id, to: Slot) {
        let items = self.display_items();
        let Some(from) = items
            .iter()
            .find(|item| &item.id == grabbed)
            .map(|item| item.slot)
        else {
            return;
        };
        let dcol = to.col - from.col;
        let drow = to.row - from.row;
        if dcol == 0 && drow == 0 {
            return;
        }
        let moving: Vec<Id> = if self.selected.iter().any(|id| id == grabbed) {
            self.selected.clone()
        } else {
            vec![grabbed.clone()]
        };
        let moving_set: HashSet<Id> = moving.iter().cloned().collect();
        let mut desired = Vec::new();
        for id in &moving {
            let Some(slot) = items
                .iter()
                .find(|item| &item.id == id)
                .map(|item| item.slot)
            else {
                continue;
            };
            desired.push((
                id.clone(),
                Slot {
                    col: (slot.col + dcol).max(0),
                    row: (slot.row + drow).max(0),
                },
            ));
        }
        let mut taken: HashSet<Slot> = items
            .iter()
            .filter(|item| !moving_set.contains(&item.id))
            .map(|item| item.slot)
            .collect();
        let stack_moved = moving.iter().any(|id| matches!(id, Id::Stack(_)));
        for (id, mut slot) in desired {
            if taken.contains(&slot) {
                slot = nearest_free(&taken, slot, self.grid);
            }
            taken.insert(slot);
            self.write_slot(&id, slot);
        }
        if stack_moved && self.spread.is_some() {
            self.recompute_fan();
        }
    }

    pub fn rename_file(&mut self, from: &str, to: &str) {
        if let Some(file) = self.files.iter_mut().find(|file| file.name == from) {
            file.name = to.to_string();
        }
        if let Some(slot) = self.layout.slots.remove(&file_key(from)) {
            self.layout.slots.insert(file_key(to), slot);
        }
        for id in &mut self.selected {
            if let Id::File(name) = id {
                if name == from {
                    *name = to.to_string();
                }
            }
        }
        if let Some(Id::File(name)) = &mut self.anchor {
            if name == from {
                *name = to.to_string();
            }
        }
        for (name, _) in &mut self.fan {
            if name == from {
                *name = to.to_string();
            }
        }
    }

    pub fn command_enabled(&self, command: &str, can_paste: bool) -> bool {
        match command {
            "new-folder" | "use-stacks" => true,
            "open" => !self.selected.is_empty(),
            // With nothing selected, Get Info shows the Desktop folder.
            "get-info" => true,
            "quick-look" => self.preview_target().is_some(),
            "rename" => matches!(self.selected.as_slice(), [Id::File(_)]),
            "move-to-trash" | "cut" | "copy" => !self.file_targets().is_empty(),
            "paste" => can_paste,
            "select-all" => !self.display_items().is_empty(),
            "unmount" => self.volume_menu_word().is_some(),
            _ => false,
        }
    }

    /// What Quick Look shows: the anchor if it is a file or a volume, else the
    /// first selected file or volume. Stacks have nothing to preview.
    pub fn preview_target(&self) -> Option<&Id> {
        let previewable = |id: &&Id| !matches!(id, Id::Stack(_));
        self.anchor
            .as_ref()
            .filter(|id| self.selected.contains(id))
            .filter(previewable)
            .or_else(|| self.selected.iter().find(previewable))
    }

    /// Hold `count` free cells starting at `start`, in grid order, for files
    /// that are about to arrive.
    pub fn free_slots_from(&self, start: Slot, count: usize) -> Vec<Slot> {
        let mut taken: HashSet<Slot> = self.layout.slots.values().copied().collect();
        taken.extend(self.display_items().into_iter().map(|item| item.slot));
        let mut slots = Vec::with_capacity(count);
        let mut index = self.grid.index(start);
        while slots.len() < count {
            let slot = self.grid.free_from(&taken, index);
            index = self.grid.index(slot) + 1;
            taken.insert(slot);
            slots.push(slot);
        }
        slots
    }

    /// Place a file that is not listed yet. Ignored once the file has a cell.
    pub fn reserve_file_slot(&mut self, name: &str, slot: Slot) {
        self.layout.slots.entry(file_key(name)).or_insert(slot);
    }

    pub fn file_targets(&self) -> Vec<String> {
        let mut names = Vec::new();
        for id in &self.selected {
            match id {
                Id::File(name) => names.push(name.clone()),
                Id::Stack(group) => {
                    for file in &self.files {
                        if self.in_group(file, group) {
                            names.push(file.name.clone());
                        }
                    }
                }
                Id::Mount(_) => {}
            }
        }
        names.sort();
        names.dedup();
        names
    }

    pub fn volume_menu_word(&self) -> Option<&'static str> {
        let [Id::Mount(uri)] = self.selected.as_slice() else {
            return None;
        };
        let mount = self.mounts.iter().find(|mount| &mount.uri == uri)?;
        if mount.eject {
            Some("Eject")
        } else if mount.unmount {
            Some("Unmount")
        } else {
            None
        }
    }

    pub fn layout_text(&self) -> String {
        self.layout.to_text()
    }

    /// The unselected folder or mounted volume under a dragged icon.
    pub fn drop_target(&self, x: f64, y: f64) -> Option<Id> {
        let id = self.grid.icon_at(&self.display_items(), x, y)?;
        if self.selected.contains(&id) {
            return None;
        }
        let accepts = match &id {
            Id::File(name) => self.is_folder(name),
            Id::Mount(uri) => self.mount(uri).is_some_and(|mount| !mount.path.is_empty()),
            Id::Stack(_) => false,
        };
        accepts.then_some(id)
    }

    fn group_counts(&self) -> BTreeMap<String, (Group, usize)> {
        let mut counts: BTreeMap<String, (Group, usize)> = BTreeMap::new();
        for file in &self.files {
            let group = self.group_of(file);
            counts.entry(group.id.clone()).or_insert((group, 0)).1 += 1;
        }
        counts
    }

    fn group_count(&self, id: &str) -> usize {
        self.files
            .iter()
            .filter(|file| self.in_group(file, id))
            .count()
    }

    fn file_slot(&self, name: &str) -> Slot {
        self.layout
            .slots
            .get(&file_key(name))
            .copied()
            .unwrap_or(Slot { col: 0, row: 0 })
    }

    fn mount_slot(&self, uri: &str) -> Slot {
        self.layout
            .slots
            .get(&mount_key(uri))
            .copied()
            .unwrap_or(Slot { col: 0, row: 0 })
    }

    fn stack_slot(&self, id: &str) -> Slot {
        if let Some(slot) = self.layout.slots.get(&stack_key(id)) {
            return *slot;
        }
        self.files
            .iter()
            .filter(|file| self.in_group(file, id))
            .map(|file| self.file_slot(&file.name))
            .min_by_key(|slot| (slot.row, slot.col))
            .unwrap_or(Slot { col: 0, row: 0 })
    }

    fn assign_file_slots(&mut self) {
        let live: HashSet<String> = self.files.iter().map(|file| file_key(&file.name)).collect();
        self.layout
            .slots
            .retain(|key, _| !key.starts_with("file:") || live.contains(key));
        let mut taken: HashSet<Slot> = self.layout.slots.values().copied().collect();
        let mut names: Vec<String> = self.files.iter().map(|file| file.name.clone()).collect();
        names.sort();
        for name in names {
            let key = file_key(&name);
            if self.layout.slots.contains_key(&key) {
                continue;
            }
            let slot = self.grid.first_free(&taken);
            taken.insert(slot);
            self.layout.slots.insert(key, slot);
        }
    }

    fn assign_mount_slots(&mut self) {
        let mut taken: HashSet<Slot> = self.layout.slots.values().copied().collect();
        // A drive keeps its cell while unplugged, and a file may be moved there
        // meanwhile. The file keeps the cell and the drive takes a free one.
        let file_slots: HashSet<Slot> = self
            .files
            .iter()
            .filter_map(|file| self.layout.slots.get(&file_key(&file.name)).copied())
            .collect();
        for mount in &self.mounts {
            let key = mount_key(&mount.uri);
            if self
                .layout
                .slots
                .get(&key)
                .is_some_and(|slot| !file_slots.contains(slot))
            {
                continue;
            }
            let slot = self.grid.first_free(&taken);
            taken.insert(slot);
            self.layout.slots.insert(key, slot);
        }
    }

    fn write_slot(&mut self, id: &Id, slot: Slot) {
        match id {
            Id::File(name) => {
                self.layout.slots.insert(file_key(name), slot);
                if let Some(entry) = self.fan.iter_mut().find(|(fan_name, _)| fan_name == name) {
                    entry.1 = slot;
                }
            }
            Id::Stack(group) => {
                self.layout.slots.insert(stack_key(group), slot);
            }
            Id::Mount(uri) => {
                self.layout.slots.insert(mount_key(uri), slot);
            }
        }
    }

    fn recompute_fan(&mut self) {
        let Some(id) = self.spread.clone() else {
            self.fan.clear();
            return;
        };
        if self.group_count(&id) < 2 {
            self.spread = None;
            self.fan.clear();
            return;
        }
        self.fan = self.compute_fan(&id);
    }

    fn compute_fan(&self, id: &str) -> Vec<(String, Slot)> {
        let mut taken = HashSet::new();
        taken.insert(self.stack_slot(id));
        let counts = self.group_counts();
        for file in &self.files {
            let group = self.group_of(file).id;
            if group == id {
                continue;
            }
            let stacked = self.layout.stacks && counts.get(&group).is_some_and(|(_, n)| *n >= 2);
            if stacked {
                continue;
            }
            taken.insert(self.file_slot(&file.name));
        }
        if self.layout.stacks {
            for (other, (_, count)) in &counts {
                if *count >= 2 {
                    taken.insert(self.stack_slot(other));
                }
            }
        }
        for mount in &self.mounts {
            taken.insert(self.mount_slot(&mount.uri));
        }
        let mut names: Vec<String> = self
            .files
            .iter()
            .filter(|file| self.in_group(file, id))
            .map(|file| file.name.clone())
            .collect();
        names.sort();
        let mut index = self.grid.index(self.stack_slot(id));
        let mut fan = Vec::new();
        for name in names {
            let slot = self.grid.free_from(&taken, index + 1);
            index = self.grid.index(slot);
            taken.insert(slot);
            fan.push((name, slot));
        }
        fan
    }

    fn prune(&mut self) {
        let visible: HashSet<Id> = self
            .display_items()
            .into_iter()
            .map(|item| item.id)
            .collect();
        self.selected.retain(|id| visible.contains(id));
        if self.anchor.as_ref().is_some_and(|id| !visible.contains(id)) {
            self.anchor = None;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// The icon grid on the screen. As in Finder, column 0 is the rightmost
/// column, and the grid fills down each column before moving left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub cols: i32,
    pub rows: i32,
    pub width: i32,
}

impl Grid {
    pub fn for_area(width: i32, height: i32) -> Self {
        Self {
            cols: ((width - MARGIN_X * 2) / CELL_W).max(1),
            rows: ((height - MARGIN_TOP - MARGIN_BOTTOM) / CELL_H).max(1),
            width,
        }
    }

    fn with_cols(cols: i32) -> Self {
        let cols = cols.max(1);
        Self {
            cols,
            rows: 6,
            width: cols * CELL_W + MARGIN_X * 2,
        }
    }

    /// The top-left corner of a cell, in surface coordinates.
    pub fn origin(&self, slot: Slot) -> (f64, f64) {
        (
            f64::from(self.width - MARGIN_X - (slot.col + 1) * CELL_W),
            f64::from(MARGIN_TOP + slot.row * CELL_H),
        )
    }

    /// The cell nearest an icon whose top-left corner is at `x`, `y`.
    pub fn slot_near(&self, x: f64, y: f64) -> Slot {
        let col = ((f64::from(self.width - MARGIN_X - CELL_W) - x) / f64::from(CELL_W)).round();
        let row = ((y - f64::from(MARGIN_TOP)) / f64::from(CELL_H)).round();
        Slot {
            col: (col as i32).clamp(0, self.cols - 1),
            row: (row as i32).clamp(0, self.rows - 1),
        }
    }

    pub fn icon_at(&self, items: &[Item], x: f64, y: f64) -> Option<Id> {
        items.iter().find_map(|item| {
            let (ix, iy) = self.origin(item.slot);
            let inside =
                x >= ix && x < ix + f64::from(CELL_W) && y >= iy && y < iy + f64::from(CELL_H);
            inside.then(|| item.id.clone())
        })
    }

    pub fn icons_in_rect(&self, items: &[Item], x: f64, y: f64, w: f64, h: f64) -> Vec<Id> {
        let (x1, x2) = if w < 0.0 { (x + w, x) } else { (x, x + w) };
        let (y1, y2) = if h < 0.0 { (y + h, y) } else { (y, y + h) };
        let mut hits: Vec<Item> = items
            .iter()
            .filter(|item| {
                let (ix, iy) = self.origin(item.slot);
                let ix2 = ix + f64::from(CELL_W);
                let iy2 = iy + f64::from(CELL_H);
                ix < x2 && ix2 > x1 && iy < y2 && iy2 > y1
            })
            .cloned()
            .collect();
        hits.sort_by_key(|item| self.index(item.slot));
        hits.into_iter().map(|item| item.id).collect()
    }

    /// A cell's place in fill order. Cells past the bottom of the screen
    /// come after every visible cell.
    fn index(&self, slot: Slot) -> i32 {
        if slot.row < self.rows && slot.col < self.cols {
            slot.col * self.rows + slot.row
        } else {
            self.cols * self.rows + (slot.row - self.rows).max(0) * self.cols + slot.col
        }
    }

    fn slot(&self, index: i32) -> Slot {
        let visible = self.cols * self.rows;
        if index < visible {
            Slot {
                col: index / self.rows,
                row: index % self.rows,
            }
        } else {
            let extra = index - visible;
            Slot {
                col: extra % self.cols,
                row: self.rows + extra / self.cols,
            }
        }
    }

    fn first_free(&self, taken: &HashSet<Slot>) -> Slot {
        self.free_from(taken, 0)
    }

    fn free_from(&self, taken: &HashSet<Slot>, start: i32) -> Slot {
        (start.max(0)..)
            .map(|index| self.slot(index))
            .find(|slot| !taken.contains(slot))
            .unwrap_or_default()
    }
}

pub fn new_folder_name(existing: &[String]) -> String {
    available_name(existing, "untitled folder")
}

pub fn available_name(existing: &[String], name: &str) -> String {
    if !existing.iter().any(|item| item == name) {
        return name.to_string();
    }
    let (stem, ext) = split_name(name);
    for index in 2..10_000 {
        let candidate = if ext.is_empty() {
            format!("{stem} {index}")
        } else {
            format!("{stem} {index}.{ext}")
        };
        if !existing.iter().any(|item| item == &candidate) {
            return candidate;
        }
    }
    format!("{name}-copy")
}

fn split_name(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, ext),
        _ => (name, ""),
    }
}

fn file_key(name: &str) -> String {
    format!("file:{name}")
}

fn stack_key(group: &str) -> String {
    format!("stack:{group}")
}

fn mount_key(uri: &str) -> String {
    format!("mount:{uri}")
}

/// The free cell closest to `near`, so an icon dropped on another one lands
/// beside it rather than back at the top of the grid.
fn nearest_free(taken: &HashSet<Slot>, near: Slot, grid: Grid) -> Slot {
    let cols = grid.cols;
    let near = Slot {
        col: near.col.clamp(0, cols - 1),
        row: near.row.max(0),
    };
    for radius in 0..10_000 {
        let mut best: Option<(i32, Slot)> = None;
        for row in (near.row - radius).max(0)..=near.row + radius {
            for col in (near.col - radius).max(0)..=(near.col + radius).min(cols - 1) {
                let ring = (row - near.row).abs().max((col - near.col).abs());
                if ring != radius {
                    continue;
                }
                let slot = Slot { col, row };
                if taken.contains(&slot) {
                    continue;
                }
                let dist = (row - near.row).pow(2) + (col - near.col).pow(2);
                if best.is_none_or(|(found, _)| dist < found) {
                    best = Some((dist, slot));
                }
            }
        }
        if let Some((_, slot)) = best {
            return slot;
        }
    }
    grid.first_free(taken)
}

fn grid_order(items: &[Item], grid: Grid) -> Vec<Id> {
    let mut items = items.to_vec();
    items.sort_by(|a, b| (grid.index(a.slot), &a.label).cmp(&(grid.index(b.slot), &b.label)));
    items.into_iter().map(|item| item.id).collect()
}

fn neighbor(items: &[Item], from: Slot, current: &Id, dir: Dir) -> Option<Id> {
    let mut best: Option<(i32, i32, Id)> = None;
    for item in items {
        if &item.id == current {
            continue;
        }
        let (ok, primary, secondary) = match dir {
            // Column 0 is on the right, so left is a higher column.
            Dir::Left => (
                item.slot.col > from.col,
                item.slot.col - from.col,
                (item.slot.row - from.row).abs(),
            ),
            Dir::Right => (
                item.slot.col < from.col,
                from.col - item.slot.col,
                (item.slot.row - from.row).abs(),
            ),
            Dir::Up => (
                item.slot.row < from.row,
                from.row - item.slot.row,
                (item.slot.col - from.col).abs(),
            ),
            Dir::Down => (
                item.slot.row > from.row,
                item.slot.row - from.row,
                (item.slot.col - from.col).abs(),
            ),
        };
        if !ok {
            continue;
        }
        let replace = best
            .as_ref()
            .is_none_or(|found| (primary, secondary) < (found.0, found.1));
        if replace {
            best = Some((primary, secondary, item.id.clone()));
        }
    }
    best.map(|found| found.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, kind: Kind) -> FileRec {
        FileRec {
            name: name.to_string(),
            mime: kind.mime().to_string(),
            kind,
            times: Times::default(),
        }
    }

    fn mount(uri: &str, name: &str) -> MountRec {
        MountRec {
            uri: uri.to_string(),
            name: name.to_string(),
            path: format!("/run/media/kevin/{name}"),
            eject: true,
            unmount: true,
        }
    }

    #[test]
    fn kinds_follow_content_type() {
        assert_eq!(
            Kind::from_content_type("inode/directory", false),
            Kind::Folder
        );
        assert_eq!(Kind::from_content_type("text/plain", true), Kind::Folder);
        assert_eq!(Kind::from_content_type("image/png", false), Kind::Image);
        assert_eq!(Kind::from_content_type("application/pdf", false), Kind::Pdf);
        assert_eq!(Kind::from_content_type("audio/mpeg", false), Kind::Music);
        assert_eq!(Kind::from_content_type("video/mp4", false), Kind::Video);
        assert_eq!(
            Kind::from_content_type("application/zip", false),
            Kind::Archive
        );
        assert_eq!(
            Kind::from_content_type("application/x-desktop", false),
            Kind::Application
        );
        assert_eq!(Kind::from_content_type("text/plain", false), Kind::Document);
        assert_eq!(
            Kind::from_content_type("application/octet-stream", false),
            Kind::Other
        );
    }

    #[test]
    fn mounts_are_media_not_the_system_disk() {
        let hidden = |root: &str| MountFacts {
            shadowed: false,
            root: root.to_string(),
            removable: false,
            optical: false,
            network: false,
        };
        assert!(!show_mount(&hidden("/")));
        assert!(!show_mount(&hidden("/boot")));
        assert!(!show_mount(&hidden("/sysroot")));
        assert!(!show_mount(&hidden("/home/kevin")));
        assert!(show_mount(&hidden("/run/media/kevin/USB")));
        assert!(show_mount(&hidden("/media/cdrom")));
        assert!(show_mount(&hidden("/mnt/data")));
        assert!(!show_mount(&MountFacts {
            shadowed: true,
            root: "/run/media/kevin/USB".to_string(),
            removable: true,
            optical: false,
            network: false,
        }));
        assert!(show_mount(&MountFacts {
            shadowed: false,
            root: "/srv/disk".to_string(),
            removable: true,
            optical: false,
            network: false,
        }));
        assert!(show_mount(&MountFacts {
            shadowed: false,
            root: String::new(),
            removable: false,
            optical: true,
            network: false,
        }));
        assert!(show_mount(&MountFacts {
            shadowed: false,
            root: "smb://server/share".to_string(),
            removable: false,
            optical: false,
            network: true,
        }));
    }

    #[test]
    fn new_files_take_the_first_free_slot_and_round_trip() {
        let mut model = Model::new(3);
        model.set_files(vec![
            file("b.txt", Kind::Document),
            file("a.txt", Kind::Document),
        ]);
        let items = model.display_items();
        let slot = |name: &str| items.iter().find(|item| item.label == name).unwrap().slot;
        assert_eq!(slot("a.txt"), Slot { col: 0, row: 0 });
        assert_eq!(slot("b.txt"), Slot { col: 0, row: 1 });
        let text = model.layout_text();
        let mut again = Model::new(3);
        again.set_layout(Layout::parse(&text));
        again.set_files(vec![
            file("a.txt", Kind::Document),
            file("b.txt", Kind::Document),
        ]);
        assert_eq!(again.layout_text(), text);
        again.set_files(vec![
            file("a.txt", Kind::Document),
            file("b.txt", Kind::Document),
            file("c.txt", Kind::Document),
        ]);
        let items = again.display_items();
        assert_eq!(
            items
                .iter()
                .find(|item| item.label == "c.txt")
                .unwrap()
                .slot,
            Slot { col: 0, row: 2 }
        );
    }

    #[test]
    fn the_grid_starts_top_right_and_fills_down() {
        let grid = Grid::for_area(1000, 40 + 3 * CELL_H + 80);
        assert_eq!((grid.cols, grid.rows), (8, 3));
        let (x, y) = grid.origin(Slot { col: 0, row: 0 });
        assert_eq!((x, y), (f64::from(1000 - 16 - CELL_W), 40.0));
        assert!(grid.origin(Slot { col: 1, row: 0 }).0 < x);
        assert_eq!(grid.slot_near(x + 5.0, y + 5.0), Slot { col: 0, row: 0 });
        let mut taken = HashSet::new();
        let order: Vec<Slot> = (0..4)
            .map(|_| {
                let slot = grid.first_free(&taken);
                taken.insert(slot);
                slot
            })
            .collect();
        assert_eq!(
            order,
            vec![
                Slot { col: 0, row: 0 },
                Slot { col: 0, row: 1 },
                Slot { col: 0, row: 2 },
                Slot { col: 1, row: 0 },
            ]
        );
    }

    #[test]
    fn click_modifiers_and_rubber_band() {
        let mut model = Model::new(3);
        model.set_files(vec![
            file("a", Kind::Other),
            file("b", Kind::Other),
            file("c", Kind::Other),
        ]);
        let items = model.display_items();
        let id = |name: &str| {
            items
                .iter()
                .find(|item| item.label == name)
                .unwrap()
                .id
                .clone()
        };
        model.plain_click(&id("b"));
        model.ctrl_click(&id("a"));
        assert_eq!(model.selection().len(), 2);
        model.ctrl_click(&id("a"));
        assert_eq!(model.selection(), &[id("b")]);
        model.shift_click(&id("c"));
        assert_eq!(model.selection(), &[id("b"), id("c")]);
        let (x, _) = model.grid().origin(Slot { col: 0, row: 0 });
        let hits = model
            .grid()
            .icons_in_rect(&items, x - 2.0, 0.0, 120.0, 150.0);
        assert_eq!(hits, vec![id("a"), id("b")]);
        model.select_ids(hits);
        model.arrow(Dir::Down);
        assert_eq!(model.selection().len(), 1);
    }

    #[test]
    fn stacks_group_by_kind_and_spread_in_place() {
        let mut model = Model::new(4);
        model.set_files(vec![
            file("one.png", Kind::Image),
            file("two.png", Kind::Image),
            file("notes.txt", Kind::Document),
        ]);
        model.set_mounts(vec![mount("file:///run/media/kevin/USB", "USB")]);
        assert!(model.toggle_stacks());
        let labels: Vec<_> = model
            .display_items()
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(labels.contains(&"Images".to_string()));
        assert!(!labels.iter().any(|label| label == "one.png"));
        assert!(labels.contains(&"notes.txt".to_string()));
        assert!(labels.contains(&"USB".to_string()));
        model.toggle_spread("image");
        let labels: Vec<_> = model
            .display_items()
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(labels.contains(&"one.png".to_string()));
        assert!(labels.contains(&"two.png".to_string()));
        assert!(labels.contains(&"Images".to_string()));
        model.click_background();
        assert!(model.spread().is_none());
        assert!(model.selection().is_empty());
    }

    #[test]
    fn commands_follow_the_selection() {
        let mut model = Model::new(3);
        model.set_files(vec![
            file("a.txt", Kind::Document),
            file("b.png", Kind::Image),
        ]);
        model.set_mounts(vec![mount("file:///run/media/kevin/USB", "USB")]);
        assert!(model.command_enabled("new-folder", false));
        assert!(model.command_enabled("get-info", false));
        assert!(!model.command_enabled("open", false));
        assert!(!model.command_enabled("paste", false));
        assert!(model.command_enabled("paste", true));
        let file_id = Id::File("a.txt".to_string());
        model.plain_click(&file_id);
        assert!(model.command_enabled("rename", false));
        assert!(model.command_enabled("move-to-trash", false));
        assert!(model.command_enabled("get-info", false));
        let mount_id = Id::Mount("file:///run/media/kevin/USB".to_string());
        model.plain_click(&mount_id);
        assert!(!model.command_enabled("rename", false));
        assert!(!model.command_enabled("move-to-trash", false));
        assert_eq!(model.volume_menu_word(), Some("Eject"));
        assert!(model.command_enabled("open", false));
        model.select_all();
        assert!(model.command_enabled("select-all", false));
        assert!(!model.command_enabled("rename", false));
        assert!(!model.volume_menu_word().is_some());
    }

    #[test]
    fn quick_look_skips_stacks() {
        let mut model = Model::new(3);
        model.set_files(vec![
            file("one.png", Kind::Image),
            file("two.png", Kind::Image),
            file("a.txt", Kind::Document),
        ]);
        assert!(!model.command_enabled("quick-look", false));
        let text = Id::File("a.txt".to_string());
        model.plain_click(&text);
        assert_eq!(model.preview_target(), Some(&text));
        model.toggle_stacks();
        model.plain_click(&Id::Stack("image".to_string()));
        assert!(!model.command_enabled("quick-look", false));
        model.ctrl_click(&text);
        assert_eq!(model.preview_target(), Some(&text));
    }

    #[test]
    fn dropped_files_land_where_they_were_dropped() {
        let mut model = Model::new(4);
        model.set_files(vec![file("a.txt", Kind::Document)]);
        let slots = model.free_slots_from(Slot { col: 0, row: 0 }, 2);
        assert_eq!(
            slots,
            vec![Slot { col: 0, row: 1 }, Slot { col: 0, row: 2 }]
        );
        model.reserve_file_slot("b.txt", Slot { col: 3, row: 2 });
        model.set_files(vec![
            file("a.txt", Kind::Document),
            file("b.txt", Kind::Document),
        ]);
        let items = model.display_items();
        let b = items.iter().find(|item| item.label == "b.txt").unwrap();
        assert_eq!(b.slot, Slot { col: 3, row: 2 });
    }

    #[test]
    fn a_returning_drive_does_not_cover_a_file() {
        let usb = "file:///run/media/kevin/USB";
        let mut model = Model::new(4);
        model.set_files(vec![file("a.txt", Kind::Document)]);
        model.set_mounts(vec![mount(usb, "USB")]);
        let mount_id = Id::Mount(usb.to_string());
        let file_id = Id::File("a.txt".to_string());
        let slot_of = |model: &Model, id: &Id| {
            model
                .display_items()
                .into_iter()
                .find(|item| &item.id == id)
                .unwrap()
                .slot
        };
        let drive_cell = slot_of(&model, &mount_id);
        model.set_mounts(Vec::new());
        model.plain_click(&file_id);
        model.move_selection(&file_id, drive_cell);
        assert_eq!(slot_of(&model, &file_id), drive_cell);
        model.set_mounts(vec![mount(usb, "USB")]);
        assert_eq!(slot_of(&model, &file_id), drive_cell);
        assert_ne!(slot_of(&model, &mount_id), drive_cell);
    }

    #[test]
    fn an_icon_dropped_on_another_lands_beside_it() {
        let mut model = Model::new(6);
        model.set_files(vec![
            file("a", Kind::Other),
            file("b", Kind::Other),
            file("c", Kind::Other),
        ]);
        let c = Id::File("c".to_string());
        model.plain_click(&c);
        model.move_selection(&c, Slot { col: 4, row: 3 });
        let a = Id::File("a".to_string());
        model.plain_click(&a);
        model.move_selection(&a, Slot { col: 4, row: 3 });
        let slot = model
            .display_items()
            .into_iter()
            .find(|item| item.id == a)
            .unwrap()
            .slot;
        assert_ne!(slot, Slot { col: 4, row: 3 });
        assert!((slot.col - 4).abs() <= 1 && (slot.row - 3).abs() <= 1);
    }

    #[test]
    fn icons_drop_into_folders_and_volumes() {
        let mut model = Model::new(6);
        model.set_files(vec![
            file("photo.png", Kind::Image),
            file("Folder", Kind::Folder),
            file("note.txt", Kind::Document),
        ]);
        model.set_mounts(vec![mount("file:///run/media/kevin/USB", "USB")]);
        let at = |model: &Model, id: &Id| {
            let item = model
                .display_items()
                .into_iter()
                .find(|item| &item.id == id)
                .unwrap();
            let (x, y) = model.grid().origin(item.slot);
            (x + 10.0, y + 10.0)
        };
        let photo = Id::File("photo.png".to_string());
        let folder = Id::File("Folder".to_string());
        let usb = Id::Mount("file:///run/media/kevin/USB".to_string());
        model.plain_click(&photo);
        let (x, y) = at(&model, &folder);
        assert_eq!(model.drop_target(x, y), Some(folder.clone()));
        let (x, y) = at(&model, &usb);
        assert_eq!(model.drop_target(x, y), Some(usb));
        let (x, y) = at(&model, &Id::File("note.txt".to_string()));
        assert_eq!(model.drop_target(x, y), None);
        model.ctrl_click(&folder);
        let (x, y) = at(&model, &folder);
        assert_eq!(model.drop_target(x, y), None);
    }

    #[test]
    fn date_stacks_follow_finder() {
        let at = |y, m, d, h| {
            glib::DateTime::from_local(y, m, d, h, 0, 0.0)
                .unwrap()
                .to_unix()
        };
        let now = at(2026, 10, 23, 12);
        let label = |time| date_group(time, now).label;
        assert_eq!(label(at(2026, 10, 23, 1)), "Today");
        assert_eq!(label(at(2026, 10, 22, 23)), "Yesterday");
        assert_eq!(label(at(2026, 10, 18, 12)), "Previous 7 Days");
        assert_eq!(label(at(2026, 10, 1, 12)), "Previous 30 Days");
        assert_eq!(date_group(at(2026, 3, 5, 12), now).id, "date:m03");
        assert_eq!(label(at(2024, 6, 1, 12)), "2024");
        assert_eq!(label(0), "Unknown");
        assert!(
            date_group(at(2026, 3, 5, 12), now).rank < date_group(at(2025, 12, 1, 12), now).rank
        );
        assert!(
            date_group(at(2026, 9, 1, 12), now).rank < date_group(at(2026, 3, 1, 12), now).rank
        );
    }

    #[test]
    fn group_by_date_stacks_and_round_trips() {
        let mut model = Model::new(4);
        let day = 86_400;
        let now = glib::DateTime::from_local(2026, 10, 23, 12, 0, 0.0)
            .unwrap()
            .to_unix();
        model.set_now(now);
        let dated = |name: &str, kind: Kind, modified: i64| FileRec {
            times: Times {
                modified,
                ..Times::default()
            },
            ..file(name, kind)
        };
        model.set_files(vec![
            dated("a.png", Kind::Image, now - 60),
            dated("b.txt", Kind::Document, now - 120),
            dated("old.txt", Kind::Document, now - 400 * day),
        ]);
        model.toggle_stacks();
        assert!(model.set_group_by(GroupBy::Modified));
        let labels: Vec<_> = model
            .display_items()
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(labels.contains(&"Today".to_string()));
        assert!(labels.contains(&"old.txt".to_string()));
        assert!(!labels.contains(&"a.png".to_string()));
        model.toggle_spread("date:today");
        assert_eq!(model.spread(), Some("date:today"));
        let text = model.layout_text();
        assert!(text.contains("group=modified"));
        assert_eq!(Layout::parse(&text).group_by, GroupBy::Modified);
    }

    #[test]
    fn folder_names_avoid_collisions() {
        assert_eq!(new_folder_name(&[]), "untitled folder");
        assert_eq!(
            new_folder_name(&["untitled folder".to_string()]),
            "untitled folder 2"
        );
        assert_eq!(
            available_name(&["notes.txt".to_string()], "notes.txt"),
            "notes 2.txt"
        );
    }
}
