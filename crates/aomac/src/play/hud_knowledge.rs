//! KnowledgeView_c (GUI 100cefe4), independent reference icons, not inventory slots.
use super::{dvalue::{DValues, Variant, CAT_CHAR}, hud_rollup::Rollup, hud_stats::{HudStats, inv_grid as grid}, references::{Reference, ReferenceStore}};
use ao_gui::{CanvasItem, CanvasTip, Event, GfxId, Gui, InputEvent, MouseButton, WindowId};
use std::path::Path;

const CONFIG: &str = "KnowledgeViewConfig";
const INFO: u32 = 0x4b4e0000;
const DELETE: u32 = INFO + 1;
const ICON: f32 = 48.0;

type Icon = (GfxId, u32, u32);

pub(super) struct HudKnowledge {
    store: ReferenceStore,
    loaded: bool,
    read_error: bool,
    identities: std::collections::BTreeMap<u32,(u32,u32)>,
    next_dummy: u32,
    positions: grid::PositionMap,
    win: Option<WindowId>,
    cols: usize,
    layout_ready: bool,
    gap: f32,
    dirty: bool,
    urls: Vec<String>,
    context: Option<u32>,
    press: Option<(u32, f32, f32)>,
    dragging: bool,
    ghost: Option<WindowId>,
    icons: Vec<(u32, Option<Icon>)>,
    closed: bool,
    qualified: bool,
    texts: ao_formats::screens::TextDb,
}

impl HudKnowledge {
    pub fn new(dir: &Path) -> anyhow::Result<Self> {
        Ok(Self { store: Default::default(), loaded: false, read_error: false, identities: Default::default(), next_dummy: 1, positions: Default::default(), win: None, cols: 2, layout_ready: false, gap: grid::SPACING, dirty: true, urls: vec![], context: None, press: None, dragging: false, ghost: None, icons: vec![], closed: false, qualified: false, texts: ao_formats::screens::TextDb::load(dir)? })
    }

    pub fn open(&mut self, gui: &mut Gui, rollup: &mut Rollup) {
        if self.win.is_some() { return; }
        // GUI 100d694e returns Point(167,167); default MultiListView icon is 47 inclusive.
        let xml = format!("<root>{}</root>", super::hud_listview::scroll_xml(167.0, 167.0));
        match rollup.open_page(gui, "knowledge_window", "Knowledge", &xml, 168.0) {
            Ok(w) => { gui.set_window_context(w,true); self.win = Some(w); self.dirty = true; }
            Err(e) => eprintln!("hud: Knowledge: {e:#}"),
        }
    }

    pub fn close(&mut self, gui: &mut Gui, d: &mut DValues) {
        self.save(d);
        if let Some(w) = self.win.take() { gui.close_window(w); }
        if let Some(w) = self.ghost.take() { gui.close_window(w); }
        self.press = None;
        self.dragging = false;
    }

    fn identity(&self, key: u32) -> Option<(u32, u32)> {
        self.identities.get(&key).copied()
    }

    fn sync_identities(&mut self) {
        self.identities.retain(|k,_| self.store.get(*k).is_some());
        for (key,r) in self.store.entries() {
            self.identities.entry(key).or_insert_with(|| match r {
                Reference::Item {..} => {
                    let id=(0xc788,self.next_dummy);
                    self.next_dummy=self.next_dummy.wrapping_add(1);
                    id
                }
                Reference::Character {identity_type,identity_id,..} => (*identity_type,*identity_id),
            });
        }
    }

    pub fn load(&mut self, d: &DValues) {
        let Some(dir) = d.char_dir() else { return; };
        match ReferenceStore::load(&dir.join("References.bin"), 20) {
            Ok(store) => self.store = store,
            Err(e) => { self.read_error = true; eprintln!("prefs: References.bin: {e}"); return; }
        }
        self.loaded = true;
        self.read_error = false;
        self.layout_ready = false;
        self.identities.clear();
        self.next_dummy = 1;
        self.sync_identities();
        self.positions = Default::default();
        let keys: Vec<_> = self.store.entries().map(|(k, _)| k).collect();
        let mut saved = Vec::new();
        let config = match d.get(CONFIG) {
            Some(Variant::Archive(src)) => ao_gui::xml::parse(src).ok(),
            _ => None,
        };
        if let Some(root) = config {
            let mut id = None;
            for e in &root.children {
                match e.attr("name") {
                    Some("item_id") => id = pair(e.attr("value").unwrap_or(""), "Identity"),
                    Some("item_pos") => {
                        if let (Some(id), Some((x,y))) = (id.take(), pair(e.attr("value").unwrap_or(""), "IPoint")) {
                            saved.push((id,(x as usize,y as usize)));
                        }
                    }
                    _ => {}
                }
            }
        }
        // GUI 100cecf1 restores ordered pairs only until the first mismatch.
        let mut matches = true;
        for (i,&key) in keys.iter().enumerate() {
            let mut prefer = saved.get(i).filter(|(id,(x,y))| self.identity(key) == Some(*id) && *x < 50 && *y < 50).map(|(_,p)| *p);
            matches &= prefer.is_some();
            if !matches { prefer = None; }
            self.positions.sync(&keys[..=i], self.cols, &mut prefer);
        }
        self.dirty = true;
    }

    pub fn save(&self, d: &mut DValues) {
        if !self.loaded || self.read_error { return; }
        if let Err(e) = d.char_dir().map_or(Ok(()), |dir| std::fs::create_dir_all(dir).and_then(|()| self.store.save(&dir.join("References.bin")))) {
            eprintln!("prefs: References.bin: {e}");
        }
        let mut root = match d.get(CONFIG) {
            Some(Variant::Archive(src)) => ao_gui::xml::parse(src).ok(),
            _ => None,
        }.unwrap_or_else(|| ao_gui::xml::Element { name: "Archive".into(), attrs: vec![("name".into(),CONFIG.into()),("code".into(),"0".into())], children: vec![] });
        root.children.retain(|e| !matches!(e.attr("name"), Some("item_id" | "item_pos")));
        for (key, _) in self.store.entries() {
            if let (Some((kind,id)), Some((x,y))) = (self.identity(key), self.positions.get(key)) {
                for (tag,name,value) in [("Identity","item_id",format!("Identity({kind},{id})")),("IPoint","item_pos",format!("IPoint({x},{y})"))] {
                    root.children.push(ao_gui::xml::Element { name: tag.into(), attrs: vec![("name".into(),name.into()),("value".into(),value)], children: vec![] });
                }
            }
        }
        let value = Variant::Archive(super::dvalue::element_xml(&root));
        if d.exists(CONFIG) { d.set(CONFIG,value); }
        else { d.add(CONFIG,value,true,CAT_CHAR,None,None,false); }
    }

    pub fn add_character(&mut self, kind: u32, id: u32, html: Vec<u8>, d: &mut DValues) {
        if self.read_error { return; }
        match self.store.add_character(kind,id,html) {
            Ok(_) => { self.loaded = true; self.sync_positions(None); self.dirty = true; self.save(d); }
            Err(e) => eprintln!("Knowledge: character reference: {e}"),
        }
    }

    fn sync_positions(&mut self, mut prefer: Option<(usize,usize)>) {
        self.sync_identities();
        let keys: Vec<_> = self.store.entries().map(|(k,_)| k).collect();
        self.positions.sync(&keys,self.cols,&mut prefer);
    }

    pub fn drop_item(&mut self, gui: &Gui, item: ao_net::n3::world::AcgItem, x: f32, y: f32, d: &mut DValues) -> bool {
        if self.read_error { return false; }
        let Some(cell) = self.cell_at(gui,x,y) else { return false; };
        let prefer = self.positions.at(cell).is_none().then_some(cell);
        match self.store.add_item(item.low_id as u32,item.high_id as u32,item.level as u32) {
            Ok(_) => { self.loaded = true; self.sync_positions(prefer); self.dirty = true; self.save(d); }
            Err(e) => eprintln!("Knowledge: item reference: {e}"),
        }
        true
    }

    fn cell_at(&self, gui: &Gui, x: f32, y: f32) -> Option<(usize,usize)> {
        let w = self.win?;
        if gui.pointer_owner(x,y) != Some(w) { return None; }
        let r = gui.view_rect(w,"lv_scroll")?;
        if x < r.l || x > r.r || y < r.t || y > r.b { return None; }
        let r = gui.view_rect(w,"knowledge_grid")?;
        let x = x-r.l-grid::BORDER;
        let y = y-r.t-grid::BORDER;
        if x < 0.0 || y < 0.0 { return None; }
        let col = (x/(ICON+self.gap)).floor() as usize;
        let row = (y/(ICON+grid::SPACING)).floor() as usize;
        (col < self.cols && row < 50).then_some((col,row))
    }

    pub fn icon_at(&self, gui: &Gui, x: f32, y: f32) -> bool {
        self.cell_at(gui,x,y).is_some_and(|cell| {
            let Some(w) = self.win else { return false; };
            let Some(r) = gui.view_rect(w,"knowledge_grid") else { return false; };
            let (cx,cy) = grid::cell_origin(cell.0,cell.1,(self.gap,grid::SPACING));
            self.positions.at(cell).is_some() && x >= r.l+cx && x < r.l+cx+ICON && y >= r.t+cy && y < r.t+cy+ICON
        })
    }

    fn inspect(&mut self, key: u32) {
        if let Some(r) = self.store.get(key) {
            self.urls.push(match r {
                Reference::Item { low,high,ql } => format!("itemref://{low}/{high}/{ql}"),
                Reference::Character { identity_type,identity_id,name } => format!("charref://{identity_type}/{identity_id}/{}",String::from_utf8_lossy(name)),
            });
        }
    }

    pub fn input(&mut self, gui: &mut Gui, ev: &InputEvent, mods: ao_gui::Modifiers, d: &mut DValues) {
        match *ev {
            InputEvent::MouseDown {x,y,button:MouseButton::Left} => {
                self.qualified = false;
                self.press = None;
                if !self.icon_at(gui,x,y) { return; }
                if let Some(key) = self.cell_at(gui,x,y).and_then(|c| self.positions.at(c)) {
                    self.press = Some((key,x,y));
                    self.dragging = false;
                }
            }
            InputEvent::MouseMove {x,y} => {
                if let Some((key,px,py)) = self.press {
                    self.dragging |= (x-px).hypot(y-py) > 3.0;
                    if self.dragging && !mods.shift {
                        if self.ghost.is_none() {
                            let xml = "<root><CanvasView name=\"icon\" min_size=\"Point(47,47)\" max_size=\"Point(47,47)\"/></root>";
                            if let Ok(w) = gui.open_window_xml("ReferenceDrag",xml,(x as i32-16,y as i32-16),ao_gui::WindowSize::Preferred) {
                                let icon = self.icons.iter().find(|(k,_)| *k == key).and_then(|(_,i)| *i);
                                let mut images=Vec::with_capacity(2);
                                icon_items(gui,&mut images,icon,0.0,0.0);
                                gui.set_canvas(w,"icon",images);
                                self.ghost=Some(w);
                            }
                        }
                        if let Some(w) = self.ghost { gui.set_window_pos(w,(x as i32-16,y as i32-16)); }
                    }
                }
            }
            InputEvent::MouseUp {x,y,button:MouseButton::Left} => {
                if let Some(w) = self.ghost.take() { gui.close_window(w); }
                if let Some((key,_,_)) = self.press.take() {
                    let to = self.cell_at(gui,x,y);
                    if !self.dragging && (mods.shift || self.qualified)
                        && to.and_then(|p| self.positions.at(p)) == Some(key)
                    { self.inspect(key); }
                    else if let Some((from,to)) = to.filter(|to| self.dragging && self.positions.at(*to).is_none()).and_then(|to| self.positions.get(key).map(|from| (from,to))) {
                        self.positions.move_cell(from,to); self.dirty = true; self.save(d);
                    }
                }
                self.dragging = false;
            }
            InputEvent::Key {key:ao_gui::Key::Escape,pressed:true,..} => {
                if let Some(w) = self.ghost.take() { gui.close_window(w); }
                self.press=None;
                self.dragging=false;
            }
            _ => {}
        }
    }

    pub fn event(&mut self, gui: &mut Gui, ev: &Event, screen: (u32,u32), d: &mut DValues) -> bool {
        let Some(w) = self.win else { return false; };
        match ev {
            Event::CloseRequested {window} if *window == w => { self.close(gui,d); self.closed = true; }
            Event::WindowFrame {window,..} if *window == w => self.dirty = true,
            Event::CanvasPress {window,view,button:MouseButton::Left,clicks,..} if *window == w && view == "knowledge_grid" => {
                self.qualified = *clicks >= 2;
            }
            Event::ContextMenu {window,x,y,..} if *window == w => {
                self.context = self.cell_at(gui,*x as f32,*y as f32).and_then(|c| self.positions.at(c));
                if self.context.is_some() {
                    let text = |key| self.texts.by_key(10000,key).unwrap_or_default();
                    gui.open_menu((*x,*y),(screen.0 as i32,screen.1 as i32),vec![ao_gui::MenuItem::entry(INFO,&text("Look@Item")),ao_gui::MenuItem::entry(DELETE,&text("DeleteReference"))]);
                }
            }
            Event::MenuPicked {id} if matches!(*id, INFO | DELETE) => {
                if let Some(key) = self.context.take() {
                    if *id == INFO { self.inspect(key); }
                    else { self.store.remove(key); self.sync_positions(None); self.dirty = true; self.save(d); }
                }
            }
            _ => return false,
        }
        true
    }

    pub fn update(&mut self, gui: &mut Gui, stats: &mut HudStats) {
        let Some(w) = self.win else { return; };
        let viewport = gui.view_rect(w,"lv_scroll");
        let width = viewport.map_or(167.0,|r| r.width());
        let (cols,gap) = grid::columns(width);
        if cols != self.cols {
            if self.layout_ready { self.positions.reflow(cols); }
            self.cols=cols; self.dirty=true;
        }
        self.layout_ready=true;
        if gap != self.gap { self.gap=gap; self.dirty=true; }
        if !self.dirty { return; }
        self.sync_positions(None);
        let rows = self.store.entries().filter_map(|(k,_)| self.positions.get(k)).map(|(_,y)|y+1).max().unwrap_or(1);
        let height = (2.0*grid::BORDER + rows as f32*ICON + rows.saturating_sub(1) as f32*grid::SPACING).max(viewport.map_or(168.0,|r| r.height()+1.0));
        let xml = format!("<root><CanvasView name=\"knowledge_grid\" min_size=\"Point({}, {})\" max_size=\"Point({}, {})\"/></root>",width,height-1.0,width,height-1.0);
        gui.remove_children(w,"lv_content");
        if let Err(e) = gui.add_view_xml(w,"lv_content","KnowledgeGrid",&xml) { eprintln!("Knowledge: {e:#}"); return; }
        let mut images = vec![];
        let mut tips = vec![];
        self.icons.clear();
        // GUI 10134023: icon-size 1 and an unbounded list use background 0x11d.
        let id=GfxId(0x11d); let (sw,sh)=gui.gfx().size(id);
        let painted_rows=((height-grid::BORDER)/(ICON+grid::SPACING)).ceil() as usize;
        for row in 0..painted_rows {
            for col in 0..self.cols {
                let (x,y)=grid::cell_origin(col,row,(self.gap,grid::SPACING));
                let x=x-((sw as f32-ICON)*0.5).floor();
                let y=y-((sh as f32-ICON)*0.5).floor();
                images.push(CanvasItem::Image {id,src:[0.0,0.0,sw as f32,sh as f32],dst:[x,y,x+sw as f32,y+sh as f32],alpha:1.0});
            }
        }
        for (key,r) in self.store.entries() {
            let Some((col,row)) = self.positions.get(key) else { continue; };
            let (x,y) = grid::cell_origin(col,row,(self.gap,grid::SPACING));
            let (title,icon) = match r {
                Reference::Item {low,high,ql} => {
                    let item=ao_net::n3::world::AcgItem {low_id:*low as i32,high_id:*high as i32,level:*ql as i32};
                    stats.shop_info(gui,item).map(|(name,_,_,icon)| (name,icon.map(|id| { let (w,h)=gui.gfx().size(id); (id,w,h) }))).unwrap_or_default()
                }
                Reference::Character {name,..} => (character_name(name),Some((GfxId(0xb8),gui.gfx().size(GfxId(0xb8)).0,gui.gfx().size(GfxId(0xb8)).1))),
            };
            self.icons.push((key,icon));
            icon_items(gui,&mut images,icon,x,y);
            tips.push(CanvasTip {rect:[x,y,x+ICON,y+ICON],title,body:String::new()});
        }
        gui.set_canvas(w,"knowledge_grid",images);
        gui.set_canvas_tips(w,"knowledge_grid",tips);
        self.dirty=false;
    }

    pub fn take_urls(&mut self) -> Vec<String> { std::mem::take(&mut self.urls) }
    pub fn take_closed(&mut self) -> bool { std::mem::take(&mut self.closed) }
}

fn icon_items(gui: &Gui, images: &mut Vec<CanvasItem>, icon: Option<Icon>, x: f32, y: f32) {
    let (id,iw,ih) = icon.unwrap_or_else(|| { let id=GfxId(0xb9); let (w,h)=gui.gfx().size(id); (id,w,h) });
    images.push(CanvasItem::Image {id,src:[0.0,0.0,iw as f32,ih as f32],dst:[x,y,x+ICON,y+ICON],alpha:1.0});
    // ItemIconView 1003f859/1003f2ea: reference badge above the icon,
    // bottom-left, natural size at a 48px icon.
    let id=GfxId(0x15c); let (bw,bh)=gui.gfx().size(id);
    images.push(CanvasItem::Image {id,src:[0.0,0.0,bw as f32,bh as f32],dst:[x,y+ICON-bh as f32,x+bw as f32,y+ICON],alpha:1.0});
}

fn pair(s: &str, tag: &str) -> Option<(u32,u32)> {
    let s=s.strip_prefix(tag)?.strip_prefix('(')?.strip_suffix(')')?;
    let (x,y)=s.split_once(',')?;
    Some((x.trim().parse().ok()?,y.trim().parse().ok()?))
}

/// References.dll 100013b3: first newline, StripHTML, ConvertCharRefs.
fn character_name(bytes: &[u8]) -> String {
    let html=String::from_utf8_lossy(bytes);
    let mut tag=false;
    let plain: String=html.split('\n').next().unwrap_or_default().chars().filter(|&c| {
        if c=='<' {tag=true;false} else if c=='>' {tag=false;false} else {!tag}
    }).collect();
    plain.replace("&lt;","<").replace("&gt;",">").replace("&quot;","\"").replace("&apos;","'").replace("&nbsp;"," ").replace("&amp;","&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(h: &mut HudKnowledge, gui: &mut Gui, d: &mut DValues, ev: InputEvent, mods: ao_gui::Modifiers) {
        h.input(gui,&ev,mods,d);
        for event in gui.input(ev) { h.event(gui,&event,(1280,800),d); }
    }

    #[test]
    fn native_reference_grid_inspects_moves_deletes_and_restores() {
        let dir=ao_gui::client_dir();
        let Ok(mut gui)=Gui::new(&dir,None) else { return; };
        let mut d=DValues::new(&dir);
        d.open_user(&super::super::prefs::test_dir().join("knowledge"),"account",7);
        let mut h=HudKnowledge::new(&dir).unwrap();
        h.load(&d);
        let html=b"<font color=CCInfoHeadline>A&amp;B</font>\n<div>Dated snapshot</div>".to_vec();
        h.add_character(50000,42,html.clone(),&mut d);
        assert_eq!(character_name(&html),"A&B");
        let mut rollup=Rollup::new(&dir,(1280,800));
        let mut stats=HudStats::new(&dir,(1280,800)).unwrap();
        h.open(&mut gui,&mut rollup);
        h.update(&mut gui,&mut stats);
        let w=h.win.unwrap();
        let r=gui.view_rect(w,"knowledge_grid").unwrap();
        let (x,y)=(r.l+grid::BORDER+12.0,r.t+grid::BORDER+12.0);
        assert!(h.icon_at(&gui,x,y));
        assert!(!h.icon_at(&gui,r.l,r.t));
        let images=gui.canvas_items(w,"knowledge_grid");
        assert!(images.iter().any(|i| matches!(i,CanvasItem::Image {id:GfxId(0x11d),..})));
        assert!(images.iter().any(|i| matches!(i,CanvasItem::Image {id:GfxId(0xb8),dst,..} if dst[2]-dst[0] == 48.0)));
        assert!(images.iter().any(|i| matches!(i,CanvasItem::Image {id:GfxId(0x15c),dst,..} if dst[0] == grid::BORDER)));
        let shift=ao_gui::Modifiers {shift:true,..Default::default()};
        input(&mut h,&mut gui,&mut d,InputEvent::MouseDown {x,y,button:MouseButton::Left},shift);
        assert!(h.take_urls().is_empty(),"inspection is release-qualified");
        input(&mut h,&mut gui,&mut d,InputEvent::MouseUp {x,y,button:MouseButton::Left},shift);
        assert_eq!(h.take_urls(),[format!("charref://50000/42/{}",String::from_utf8_lossy(&html))]);
        assert!(h.drop_item(&gui,ao_net::n3::world::AcgItem {low_id:51018,high_id:51018,level:1},x,y,&mut d));
        h.update(&mut gui,&mut stats);
        assert_eq!(h.store.entries().count(),2);
        assert_eq!(h.identity(2),Some((0xc788,1)));
        h.inspect(2);
        assert_eq!(h.take_urls(),["itemref://51018/51018/1"]);
        let to=(x,y+ICON+grid::SPACING);
        input(&mut h,&mut gui,&mut d,InputEvent::MouseDown {x,y,button:MouseButton::Left},Default::default());
        input(&mut h,&mut gui,&mut d,InputEvent::MouseMove {x:to.0,y:to.1},Default::default());
        assert!(h.ghost.is_some());
        input(&mut h,&mut gui,&mut d,InputEvent::MouseUp {x:to.0,y:to.1,button:MouseButton::Left},Default::default());
        assert_eq!(h.positions.get(1),Some((0,1)));
        assert!(h.ghost.is_none());
        h.close(&mut gui,&mut d);
        rollup.close_page(&mut gui,"knowledge_window");
        let mut restored=HudKnowledge::new(&dir).unwrap();
        restored.load(&d);
        restored.open(&mut gui,&mut rollup);
        restored.update(&mut gui,&mut stats);
        assert_eq!(restored.positions.get(1),Some((0,1)));
        assert_eq!(restored.store.entries().count(),2);
        let rr=gui.view_rect(restored.win.unwrap(),"knowledge_grid").unwrap();
        input(&mut restored,&mut gui,&mut d,InputEvent::MouseDown {x:rr.l+grid::BORDER+12.0,y:rr.t+grid::BORDER+ICON+grid::SPACING+12.0,button:MouseButton::Right},Default::default());
        assert!(gui.menu_open());
        assert_eq!(restored.context,Some(1));
        assert!(restored.event(&mut gui,&Event::MenuPicked {id:DELETE},(1280,800),&mut d));
        gui.close_menu();
        assert_eq!(restored.store.entries().count(),1);
        assert_eq!(restored.identity(2),Some((0xc788,1)),"deletion never renumbers live dummy identities");
        let path=d.char_dir().unwrap().join("References.bin");
        assert_eq!(ReferenceStore::load(&path,20).unwrap().entries().count(),1);
        restored.close(&mut gui,&mut d);
        rollup.close_page(&mut gui,"knowledge_window");
        std::fs::write(&path,b"broken").unwrap();
        let mut bad=HudKnowledge::new(&dir).unwrap();
        bad.load(&d);
        bad.add_character(50000,42,html,&mut d);
        bad.close(&mut gui,&mut d);
        assert_eq!(std::fs::read(&path).unwrap(),b"broken","a load error must not erase existing references");
    }
}
