//! The options window's "Fixed keys" and "Key bindings" tabs (`OptionWindow_c` `FUN_100c3c44`: `Window::AppendTab("Fixed keys", HotKeyPanel_c)` when
//! `HotKeys.xml` loads, `AppendTab("Key bindings", BindView_c)`).
//!
//! * **Fixed keys** = `HotKeyPanel_c` (`FUN_100bef0a`): a `ListViewBase_c` tree built from `OptionPanel/HotKeys.xml` by `FUN_100be84b` (`<HotKeyGroup label mode>` = folder,
//!   `<HotKey down_id="id:KEY_*" label>` = `HotKeyListItem_c` row showing `InputConfig_t::GetHotkey(id)+8`, [`keys::fixed_text`]; `mode="fixed"` rows are not selectable).
//!   Open folders / the selection / the scroll offset go into the `hotkey_config` sub-archive of `OptionWindowConfig` (`FUN_100bd921`).
//! * **Key bindings** = `BindView_c` (`FUN_100bc91b`): a `MultiListView` with the columns Category (100) / Function (200) / Key (100) (flags 0xe) over the provider map
//!   (`FUN_100bc07b`: one row per provider and bound input, one row with input 0 = "NoKey" for an unbound provider) and the buttons Reset All / Add / Change / Clear.
//!   Add / Change open the "Bind Key" dialog (`FUN_100bc60d`), whose OK commits the key pressed meanwhile (`FUN_100bc229`).

use super::super::dvalue::{DValues, Variant};
use super::super::hud_dialog::Dialogs;
use super::keys::{self, Bindings, FixedKeys, Provider};
use ao_formats::screens::{TextDb, CAT_GUI, CAT_LABELS};
use ao_gui::view::ListItem;
use ao_gui::widgets::MultiCell;
use ao_gui::{xml, Event, Gui, WindowId};

pub(super) const FIXED: &str = "fixed";
pub(super) const BINDS: &str = "binds";
/// Folder icons of the options tree (`StringListViewItem_c(.., 0xd5, 0xd4)`).
const FOLDER_ICONS: (u32, u32) = (0xd5, 0xd4);
/// `MultiListView_c` feature flags 0x40 (selection) and the column flags `0xe` of `FUN_100bc91b`.
const COL_FLAGS: u32 = 0xe;
/// Text categories of the provider label (`#Key` -> `GetText(0x7db, ..)`) and group (`GetText(0x7dc, ..)`).
const CAT_FUNCTION: u32 = 0x7db;
const CAT_CATEGORY: u32 = 0x7dc;

/// `HotKeys.xml`: groups and hot keys.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Node {
    Group { label: String, fixed: bool, kids: Vec<Node> },
    Key { label: String, name: String },
}

/// `FUN_100be84b`: `HotKeyGroup` (`mode="fixed"` = not selectable) and `HotKey` (`down_id` = `id:KEY_*`) elements; other elements and hot keys without a label are skipped.
pub(super) fn parse_hotkeys(text: &str) -> Vec<Node> {
    fn walk(e: &xml::Element) -> Vec<Node> {
        e.children
            .iter()
            .filter_map(|c| {
                let label = c.attr("label")?.to_string();
                if c.name.eq_ignore_ascii_case("HotKeyGroup") {
                    Some(Node::Group { label, fixed: c.attr("mode").is_some_and(|m| m.eq_ignore_ascii_case("fixed")), kids: walk(c) })
                } else if c.name.eq_ignore_ascii_case("HotKey") {
                    Some(Node::Key { label, name: c.attr("down_id")?.strip_prefix("id:")?.to_string() })
                } else {
                    None
                }
            })
            .collect()
    }
    xml::parse(text).map(|r| walk(&r)).unwrap_or_default()
}

/// What the "Bind Key" dialog was opened for (`Add` = mode byte 0, `Change` = 1): the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bind {
    change: bool,
    row: usize,
}

/// One row of the key list: a provider and one of its inputs (0 = unbound).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub provider: u32,
    pub input: u32,
}

/// Saved state of the Fixed keys panel (`hotkey_config`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct HotkeyConfig {
    pub folders: Vec<String>,
    pub selected: String,
    pub scroll: f32,
}

pub(super) struct KeyPages {
    tree: Vec<Node>,
    providers: Vec<Provider>,
    dialogs: Dialogs<Bind>,
    /// The key pressed while the dialog is open (0 = none yet).
    pending: u32,
    rows: Vec<Row>,
    selected: Option<usize>,
    win: Option<WindowId>,
    dir: std::path::PathBuf,
    texts: Texts,
    screen: (u32, u32),
    /// The working copy of the binding table (re-read from the DValue every frame).
    table: Bindings,
    fixed_ids: Vec<(String, String)>,
}

/// The few texts the pages show (`LDBface::GetText(10000, ..)`).
struct Texts {
    no_key: String,
    ok: String,
    cancel: String,
    body: String,
}

fn label_of(texts: &TextDb, cat: u32, s: &str) -> String {
    match s.strip_prefix('#') {
        Some(k) => texts.by_key(cat, k).unwrap_or_else(|| s.to_string()),
        None => s.to_string(),
    }
}

impl KeyPages {
    pub(super) fn new(dir: &std::path::Path, texts: &TextDb, screen: (u32, u32)) -> Self {
        let tree = std::fs::read_to_string(dir.join("cd_image/gui/Default/OptionPanel/HotKeys.xml")).map(|t| parse_hotkeys(&t)).unwrap_or_default();
        let t = |k: &str| texts.by_key(CAT_GUI, k).unwrap_or_else(|| k.to_string());
        // `ShortcutBarKey` & co (text category 0x7db): `Bar %02d_%02d` with the 1-based bar (and slot), `FUN_100d9a81` (`INC ECX` before the format call)
        let fmt = |q: &str| {
            let mut it = q.split(' ');
            let key = it.next().unwrap_or("");
            let mut s = texts.by_key(CAT_FUNCTION, key).unwrap_or_else(|| key.to_string());
            for n in it {
                s = s.replacen("%02d", &format!("{:02}", n.parse::<u32>().unwrap_or(0)), 1);
            }
            s
        };
        let providers = keys::registry(&fmt);
        KeyPages {
            tree,
            providers,
            dialogs: Dialogs::default(),
            pending: 0,
            rows: vec![],
            selected: None,
            win: None,
            dir: dir.to_path_buf(),
            texts: Texts { no_key: t("NoKey"), ok: t("MsgBox_OK"), cancel: t("MsgBox_Cancel"), body: t("BindDialogText") },
            screen,
            table: Bindings::default(),
            fixed_ids: vec![],
        }
    }

    pub(super) fn set_screen(&mut self, s: (u32, u32)) {
        self.screen = s;
    }

    /// `AppendTab("Fixed keys", ..)` only exists when `HotKeys.xml` loaded.
    pub(super) fn has_fixed(&self) -> bool {
        !self.tree.is_empty()
    }

    /// The "Bind Key" dialog is open: the key presses go to [`KeyPages::capture`] instead of acting (`manager +0x24`).
    pub(super) fn capturing(&self) -> bool {
        self.dialogs.is_open()
    }

    /// The view XML of the two tabs (`HotKeyPanel_c` / `BindView_c`).
    pub(super) fn xml(&self) -> (String, String) {
        let fixed = format!(
            "<BorderView name=\"tab_fixed\" layout_borders=\"Rect(10,10,10,10)\"><View layout_borders=\"Rect(5,5,5,5)\"><StringListView name=\"{FIXED}\" v_scrollbar_mode=\"auto\" max_size=\"Point(16000,16000)\"/></View></BorderView>"
        );
        let btn = |n: &str, l: &str| format!("<Button name=\"{n}\" label=\"{l}\" layout_borders=\"Rect(5,5,5,5)\"/>");
        let binds = format!(
            "<View name=\"tab_bind\" view_layout=\"vertical\"><BorderView layout_borders=\"Rect(10,10,10,0)\"><View layout_borders=\"Rect(5,5,5,5)\"><MultiListView name=\"{BINDS}\" feature_flags=\"64\"/></View></BorderView>\
             <View view_layout=\"horizontal\" layout_borders=\"Rect(10,0,10,5)\"><HLayoutSpacer/>{}{}{}{}</View></View>",
            btn("kb_reset", "Reset All"),
            btn("kb_add", "Add"),
            btn("kb_change", "Change"),
            btn("kb_clear", "Clear")
        );
        (fixed, binds)
    }

    // ---------------------------------------------------------------- Fixed keys

    fn fixed_row_text(&self, name: &str, f: &FixedKeys) -> String {
        keys::fixed_text(f.get(name), &self.texts.no_key)
    }

    /// Fills the Fixed keys list: folders (`FUN_100be84b`) with their rows, the saved open folders / selection / scroll offset.
    pub(super) fn populate_fixed(&mut self, gui: &mut Gui, win: WindowId, f: &FixedKeys, cfg: &HotkeyConfig, texts: &TextDb) {
        self.win = Some(win);
        self.fixed_ids.clear();
        let mut labels = vec![];
        fn collect(nodes: &[Node], texts: &TextDb, out: &mut Vec<String>) {
            for n in nodes {
                match n {
                    Node::Group { kids, .. } => collect(kids, texts, out),
                    Node::Key { label, .. } => out.push(label_of(texts, CAT_LABELS, label)),
                }
            }
        }
        collect(&self.tree, texts, &mut labels);
        // the key column starts after the widest label (`_DAT_1026ca68`, the static maximum `FUN_100be24e` keeps) -- the exact `KeyListItemView_c` layout is UNRESOLVED
        let widest = labels.iter().map(|l| gui.text_width(ao_gui::FontId::Normal, l)).max().unwrap_or(0) as f32;
        let tree = self.tree.clone();
        self.add_nodes(gui, win, &tree, None, "", f, cfg, texts, widest, 15.0);
        for id in &cfg.folders {
            gui.list_open_folder(win, FIXED, id, true);
        }
        if !cfg.selected.is_empty() {
            gui.list_select(win, FIXED, &cfg.selected, true, false);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_nodes(&mut self, gui: &mut Gui, win: WindowId, nodes: &[Node], parent: Option<&str>, path: &str, f: &FixedKeys, cfg: &HotkeyConfig, texts: &TextDb, widest: f32, depth_x: f32) {
        for n in nodes {
            match n {
                Node::Group { label, fixed, kids } => {
                    let text = label_of(texts, CAT_LABELS, label);
                    // `FUN_100bf37c`: the id of an item is its label chained to its ancestors' (separator UNRESOLVED, '/')
                    let id = format!("{path}{text}");
                    let mut it = ListItem::new(&id, &text, FOLDER_ICONS.0, FOLDER_ICONS.1);
                    it.folder = true;
                    it.selectable = false;
                    it.open = cfg.folders.contains(&id);
                    gui.list_add(win, FIXED, parent, it);
                    self.add_nodes(gui, win, kids, Some(&id), &format!("{id}/"), f, cfg, texts, widest, depth_x + 15.0);
                    let _ = fixed;
                }
                Node::Key { label, name } => {
                    let text = label_of(texts, CAT_LABELS, label);
                    let id = format!("{path}{text}");
                    let mut it = ListItem::new(&id, &text, 0, 0);
                    it.selectable = false; // every row of the shipped file sits in a `mode="fixed"` group
                    it.aux = self.fixed_row_text(name, f);
                    it.aux_x = depth_x + widest + 15.0;
                    gui.list_add(win, FIXED, parent, it);
                    self.fixed_ids.push((id, name.clone()));
                }
            }
        }
    }

    /// The Fixed keys rows follow the `KEY_*` login prefs (the original refreshes them when the keys change, `FUN_100be1ee`).
    pub(super) fn refresh_fixed(&mut self, gui: &mut Gui, f: &FixedKeys) {
        let Some(win) = self.win else { return };
        for (id, name) in self.fixed_ids.clone() {
            let text = self.fixed_row_text(&name, f);
            if gui.list_item(win, FIXED, &id).is_some_and(|i| i.aux != text) {
                gui.list_update(win, FIXED, &id, |i| i.aux = text);
            }
        }
    }

    /// `FUN_100bd921`: the open folders, the selected row and the scroll offset of the Fixed keys list.
    pub(super) fn save_fixed(&self, gui: &Gui, texts: &TextDb) -> HotkeyConfig {
        let Some(win) = self.win else { return HotkeyConfig::default() };
        let mut cfg = HotkeyConfig::default();
        fn walk(nodes: &[Node], texts: &dyn Fn(&str) -> String, path: &str, out: &mut Vec<(String, bool)>) {
            for n in nodes {
                match n {
                    Node::Group { label, kids, .. } => {
                        let id = format!("{path}{}", texts(label));
                        out.push((id.clone(), true));
                        walk(kids, texts, &format!("{id}/"), out);
                    }
                    Node::Key { label, .. } => out.push((format!("{path}{}", texts(label)), false)),
                }
            }
        }
        let mut ids = vec![];
        let t = |l: &str| label_of(texts, CAT_LABELS, l);
        walk(&self.tree, &t, "", &mut ids);
        for (id, folder) in ids {
            match (folder, gui.list_item(win, FIXED, &id)) {
                (true, Some(i)) if i.open => cfg.folders.push(id),
                (false, Some(i)) if i.selected => cfg.selected = id,
                _ => {}
            }
        }
        cfg
    }

    // ---------------------------------------------------------------- Key bindings

    fn provider(&self, hash: u32) -> Option<&Provider> {
        self.providers.iter().find(|p| p.hash == hash)
    }

    /// `FUN_100bc07b`: rebuilds the list from the provider map (ascending hash) and the table.
    pub(super) fn populate_binds(&mut self, gui: &mut Gui, win: WindowId, b: &Bindings, texts: &TextDb) {
        self.win = Some(win);
        self.table = b.clone();
        if gui.multi_columns(win, BINDS).is_empty() {
            for (i, (label, w)) in [("Category", 100.0), ("Function", 200.0), ("Key", 100.0)].into_iter().enumerate() {
                gui.multi_add_column(win, BINDS, i as i32, label, w, COL_FLAGS);
            }
        }
        self.rebuild(gui, texts);
    }

    fn rebuild(&mut self, gui: &mut Gui, texts: &TextDb) {
        let Some(win) = self.win else { return };
        self.rows.clear();
        for p in &self.providers {
            let inputs = self.table.inputs(p.hash);
            if inputs.is_empty() {
                self.rows.push(Row { provider: p.hash, input: 0 });
            }
            self.rows.extend(inputs.into_iter().map(|input| Row { provider: p.hash, input }));
        }
        gui.multi_clear(win, BINDS);
        for (i, r) in self.rows.iter().enumerate() {
            let p = self.providers.iter().find(|p| p.hash == r.provider).unwrap();
            let key = if r.input == 0 { self.texts.no_key.clone() } else { keys::bind_text(r.input) };
            let cells = vec![MultiCell::unsorted(&label_of(texts, CAT_CATEGORY, &p.group)), MultiCell::unsorted(&label_of(texts, CAT_FUNCTION, &p.label)), MultiCell::unsorted(&key)];
            gui.multi_add_row(win, BINDS, i as i64, cells, true);
        }
        if let Some(s) = self.selected.filter(|s| *s < self.rows.len()) {
            gui.multi_select(win, BINDS, s as i64, true, false);
        } else {
            self.selected = None;
        }
    }

    /// The rows in list order (tests).
    #[cfg(test)]
    pub(super) fn rows(&self) -> &[Row] {
        &self.rows
    }

    fn store(&mut self, d: &mut DValues) {
        self.table.version = keys::VERSION.max(self.table.version);
        d.set("KeyBindings", Variant::Archive(self.table.archive()));
    }

    /// The table in the DValue changed under us (another tab, a reset): the rows follow.
    pub(super) fn sync(&mut self, gui: &mut Gui, d: &DValues, texts: &TextDb) {
        if self.win.is_none() || self.dialogs.is_open() {
            return;
        }
        if let Some(Variant::Archive(t)) = d.get("KeyBindings") {
            let b = Bindings::from_archive(t);
            if b != self.table {
                self.table = b;
                self.rebuild(gui, texts);
            }
        }
    }

    /// The default table (`DistributedValue_c::ResetToDefault("KeyBindings")`): the archive of the shipped `CharPrefs.xml`.
    fn defaults(&self) -> Option<String> {
        let t = std::fs::read_to_string(self.dir.join("cd_image/gui/Default/CharPrefs.xml")).ok()?;
        let r = xml::parse(&t).ok()?;
        let a = r.children.iter().find(|c| c.name == "Archive" && c.attr("name") == Some("KeyBindings"))?;
        Some(Bindings::from_archive(&super::xml_text(a)).archive())
    }

    fn open_dialog(&mut self, gui: &mut Gui, change: bool) {
        let Some(row) = self.selected else { return };
        if self.dialogs.is_open() {
            return;
        }
        self.pending = 0;
        let input = self.rows[row].input;
        let body = format!("{}<br><br>{}", self.texts.body, keys::bind_text(input));
        let buttons = [self.texts.ok.clone(), self.texts.cancel.clone()];
        self.dialogs.go(gui, self.screen, Bind { change, row }, &body, &buttons);
    }

    /// `FUN_100bc417`: the key pressed while the dialog is open becomes the pending key and is shown in the dialog.
    pub(super) fn capture(&mut self, gui: &mut Gui, input: u32) {
        // Esc closes the dialog (`DialogBox_c::SlotEscPressed`) -- [INFERENCE]: the original's key slot would receive it too
        if !self.dialogs.is_open() || input & keys::KEY_MASK == 14 {
            return;
        }
        self.pending = input;
        self.dialogs.set_body(gui, &format!("{}<br><br>{}", self.texts.body, keys::bind_text(input)));
    }

    /// The middle mouse button pressed while the dialog is open (`FUN_100bc58e`: mouse inputs 5 / 9 / 0x79 / 0x7a; left clicks the dialog's buttons).
    pub(super) fn capture_mouse(&mut self, gui: &mut Gui, input: u32) {
        self.capture(gui, input);
    }

    /// `FUN_100bc229` with button 0: commits the pending key.
    fn commit(&mut self, gui: &mut Gui, d: &mut DValues, b: Bind, texts: &TextDb) {
        let Some(r) = self.rows.get(b.row).copied() else { return };
        // nothing pressed = input 0: `FUN_100bc229` has no guard (`[EBP-0x14]` starts at 0 and only the key slot sets it), so Change removes the key and binds the
        // provider to input 0 (shown as the `NoKey` text, never pressed)
        let new = self.pending;
        if !b.change && self.table.has(r.provider) {
            // Add on a function that already has a key: one more key, a new row
            self.table.add(new, r.provider);
        } else {
            // Change (or Add to an unbound row): the old key goes first; when the new one is already on the function the old stays removed
            // (`FUN_10018b3a` result ignored in the original; the list shows the table)
            self.table.remove(r.input, r.provider);
            self.table.add(new, r.provider);
        }
        self.store(d);
        self.rebuild(gui, texts);
    }

    fn clear(&mut self, gui: &mut Gui, d: &mut DValues, texts: &TextDb) {
        let Some(r) = self.selected.and_then(|s| self.rows.get(s)).copied() else { return };
        if self.provider(r.provider).is_none() || !self.table.remove(r.input, r.provider) {
            return;
        }
        self.store(d);
        self.rebuild(gui, texts);
    }

    fn reset(&mut self, gui: &mut Gui, d: &mut DValues, texts: &TextDb) {
        if let Some(a) = self.defaults() {
            d.set("KeyBindings", Variant::Archive(a));
            if let Some(Variant::Archive(t)) = d.get("KeyBindings") {
                self.table = Bindings::from_archive(t);
            }
            self.rebuild(gui, texts);
        }
    }

    /// Handles the pages' events; true when consumed.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, d: &mut DValues, texts: &TextDb) -> bool {
        let (own, done) = self.dialogs.event(gui, ev);
        if let Some((bind, button)) = done {
            if button == 0 {
                self.commit(gui, d, bind, texts);
            }
        }
        if own {
            return true;
        }
        let Some(win) = self.win else { return false };
        match ev {
            Event::MultiSelected { window, view, id, selected: true } if *window == win && view == BINDS => {
                self.selected = usize::try_from(*id).ok();
                true
            }
            Event::Clicked { window, view, .. } if *window == win => match view.as_str() {
                "kb_reset" => {
                    self.reset(gui, d, texts);
                    true
                }
                "kb_add" => {
                    self.open_dialog(gui, false);
                    true
                }
                "kb_change" => {
                    self.open_dialog(gui, true);
                    true
                }
                "kb_clear" => {
                    self.clear(gui, d, texts);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui) {
        self.dialogs.close_all(gui);
        self.win = None;
        self.selected = None;
    }

    /// Rows of the Fixed keys list (tests).
    #[cfg(test)]
    pub(super) fn fixed_rows(&self) -> usize {
        self.fixed_ids.len()
    }

    #[cfg(test)]
    pub(super) fn dialog_windows(&self) -> Vec<WindowId> {
        self.dialogs.windows()
    }

    #[cfg(test)]
    pub(super) fn select_row(&mut self, gui: &mut Gui, row: usize) {
        self.selected = Some(row);
        if let Some(w) = self.win {
            gui.multi_select(w, BINDS, row as i64, true, false);
        }
    }
}
