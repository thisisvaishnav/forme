use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::convert::FromWasmAbi;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    CanvasRenderingContext2d, Document, Element as DomElement, Event, EventTarget, FileReader,
    HtmlAnchorElement, HtmlCanvasElement, HtmlDialogElement, HtmlElement, HtmlImageElement,
    HtmlInputElement, HtmlTextAreaElement, KeyboardEvent, MessageEvent, MouseEvent, NodeList,
    PointerEvent, WebSocket, WheelEvent, Window,
};

use crate::model::{dist_to_segment, Element, Fill, History, Kind, Pt, Rect, Style, Tool};
use crate::render::{self, View, FONT};

const STORAGE_KEY: &str = "forme.scene.v1";
const MIN_ZOOM: f64 = 0.1;
const MAX_ZOOM: f64 = 8.0;
const ID_PREFIX: u64 = 1_000_000_000_000;
const PRESENCE_MS: f64 = 50.0;
const ROOM_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

fn random_token(length: usize) -> String {
    let mut token = String::with_capacity(length);
    for _ in 0..length {
        let index = (js_sys::Math::random() * ROOM_ALPHABET.len() as f64) as usize;
        token.push(ROOM_ALPHABET[index.min(ROOM_ALPHABET.len() - 1)] as char);
    }
    token
}

pub type Shared = Rc<RefCell<App>>;

#[derive(Serialize, Deserialize, Default)]
struct Scene {
    #[serde(default)]
    elements: Vec<Element>,
    #[serde(default)]
    pan: Pt,
    #[serde(default)]
    zoom: f64,
}

#[derive(Clone, Debug, Deserialize)]
struct PeerInfo {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    color: String,
    #[serde(default)]
    cursor: Option<Pt>,
}

#[derive(Clone, Debug)]
struct Peer {
    name: String,
    color: String,
    cursor: Option<Pt>,
}

#[derive(Deserialize)]
#[serde(tag = "t")]
enum ServerMsg {
    #[serde(rename = "init")]
    Init {
        #[serde(rename = "clientId")]
        client_id: u64,
        #[serde(default)]
        elements: Vec<Element>,
        #[serde(default)]
        peers: Vec<PeerInfo>,
    },
    #[serde(rename = "peers")]
    Peers {
        #[serde(default)]
        peers: Vec<PeerInfo>,
    },
    #[serde(rename = "patch")]
    Patch {
        #[serde(default)]
        upsert: Vec<Element>,
        #[serde(default)]
        delete: Vec<u64>,
    },
    #[serde(rename = "cursor")]
    Cursor {
        id: u64,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
    },
    #[serde(rename = "bye")]
    Bye { id: u64 },
    #[serde(other)]
    Other,
}

struct Collab {
    ws: Option<WebSocket>,
    generation: u64,
    connected: bool,
    ready: bool,
    room: String,
    client_id: u64,
    name: String,
    color: String,
    peers: HashMap<u64, Peer>,
    last_pub: HashMap<u64, Element>,
    last_pub_at: f64,
    last_cursor_at: f64,
    last_cursor: Option<Pt>,
    session_base: u64,
    session_seq: u64,
    retry_delay: f64,
}

impl Collab {
    fn new() -> Self {
        let colors = [
            "#e03131", "#1971c2", "#2f9e44", "#f08c00", "#9c36b5", "#0c8599", "#e8590c", "#5f3dc4",
        ];
        let color = colors[(js_sys::Math::random() * colors.len() as f64) as usize].to_string();
        let name = format!(
            "Guest {:04}",
            (js_sys::Math::random() * 9000.0) as u32 + 1000
        );
        Self {
            ws: None,
            generation: 0,
            connected: false,
            ready: false,
            room: String::new(),
            client_id: 0,
            name,
            color,
            peers: HashMap::new(),
            last_pub: HashMap::new(),
            last_pub_at: 0.0,
            last_cursor_at: 0.0,
            last_cursor: None,
            session_base: 0,
            session_seq: 0,
            retry_delay: 1000.0,
        }
    }
}

#[derive(Clone)]
enum Drag {
    None,
    Pan {
        start: Pt,
        pan0: Pt,
    },
    Move {
        start: Pt,
        before: Vec<Element>,
    },
    Resize {
        handle: usize,
        start: Pt,
        before: Vec<Element>,
        bounds: Rect,
    },
    Create {
        id: u64,
        start: Pt,
        before: Vec<Element>,
    },
    Draw {
        id: u64,
        before: Vec<Element>,
    },
    Marquee {
        start: Pt,
        cur: Pt,
        additive: bool,
    },
    Erase {
        before: Vec<Element>,
    },
}

pub struct App {
    window: Window,
    document: Document,
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    elements: Vec<Element>,
    selected: Vec<u64>,
    tool: Tool,
    style: Style,
    zoom: f64,
    pan: Pt,
    history: History,
    drag: Drag,
    next_id: u64,
    space: bool,
    locked: bool,
    text_edit: Option<u64>,
    text_before: Option<Vec<Element>>,
    text_new: bool,
    clipboard: Vec<Element>,
    images: HashMap<String, HtmlImageElement>,
    show_grid: bool,
    dpr: f64,
    css_w: f64,
    css_h: f64,
    pointer: Pt,
    pending_image_pos: Option<Pt>,
    collab: Collab,
    self_ref: Weak<RefCell<App>>,
}

impl App {
    pub fn new(
        window: Window,
        document: Document,
        canvas: HtmlCanvasElement,
        ctx: CanvasRenderingContext2d,
    ) -> Self {
        Self {
            window,
            document,
            canvas,
            ctx,
            elements: Vec::new(),
            selected: Vec::new(),
            tool: Tool::Select,
            style: Style::default(),
            zoom: 1.0,
            pan: Pt::new(0.0, 0.0),
            history: History::default(),
            drag: Drag::None,
            next_id: 0,
            space: false,
            locked: false,
            text_edit: None,
            text_before: None,
            text_new: false,
            clipboard: Vec::new(),
            images: HashMap::new(),
            show_grid: false,
            dpr: 1.0,
            css_w: 1.0,
            css_h: 1.0,
            pointer: Pt::new(0.0, 0.0),
            pending_image_pos: None,
            collab: Collab::new(),
            self_ref: Weak::new(),
        }
    }

    pub fn attach(&mut self, shared: &Shared) {
        self.self_ref = Rc::downgrade(shared);
    }

    fn tool_data(&self) -> &'static str {
        match self.tool {
            Tool::Select => "select",
            Tool::Hand => "hand",
            Tool::Rectangle => "rectangle",
            Tool::Diamond => "diamond",
            Tool::Ellipse => "ellipse",
            Tool::Arrow => "arrow",
            Tool::Line => "line",
            Tool::Draw => "draw",
            Tool::Text => "text",
            Tool::Image => "image",
            Tool::Eraser => "eraser",
        }
    }

    pub fn draw(&mut self) {
        let dpr = self.window.device_pixel_ratio().max(1.0);
        let cw = self.canvas.client_width() as f64;
        let ch = self.canvas.client_height() as f64;
        let w = if cw > 2.0 {
            cw
        } else {
            self.window
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(800.0)
        };
        let h = if ch > 2.0 {
            ch
        } else {
            self.window
                .inner_height()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(600.0)
        };
        self.dpr = dpr;
        self.css_w = w;
        self.css_h = h;

        let bw = (w * dpr).round().max(1.0) as u32;
        let bh = (h * dpr).round().max(1.0) as u32;
        if self.canvas.width() != bw {
            self.canvas.set_width(bw);
        }
        if self.canvas.height() != bh {
            self.canvas.set_height(bh);
        }

        let view = View {
            pan: self.pan,
            zoom: self.zoom,
            dpr,
            w,
            h,
        };
        render::render_scene(
            &self.ctx,
            &self.elements,
            &self.images,
            &view,
            self.selection_bounds(),
            self.marquee_rect(),
            self.show_grid,
        );
        self.draw_peer_cursors();
    }

    fn draw_peer_cursors(&self) {
        if self.collab.peers.is_empty() {
            return;
        }
        let _ = self
            .ctx
            .set_transform(self.dpr, 0.0, 0.0, self.dpr, 0.0, 0.0);
        for peer in self.collab.peers.values() {
            let Some(cursor) = peer.cursor else {
                continue;
            };
            let x = self.pan.x + cursor.x * self.zoom;
            let y = self.pan.y + cursor.y * self.zoom;
            if x < -120.0 || y < -40.0 || x > self.css_w + 40.0 || y > self.css_h + 40.0 {
                continue;
            }
            self.ctx.begin_path();
            self.ctx.move_to(x, y);
            self.ctx.line_to(x + 2.5, y + 17.0);
            self.ctx.line_to(x + 7.2, y + 13.2);
            self.ctx.line_to(x + 11.5, y + 21.5);
            self.ctx.line_to(x + 15.0, y + 19.7);
            self.ctx.line_to(x + 10.8, y + 11.8);
            self.ctx.line_to(x + 16.5, y + 10.8);
            self.ctx.close_path();
            self.ctx.set_fill_style_str(&peer.color);
            self.ctx.set_stroke_style_str("#ffffff");
            self.ctx.set_line_width(1.4);
            self.ctx.fill();
            self.ctx.stroke();
            if peer.name.is_empty() {
                continue;
            }
            self.ctx.set_font("600 11px 'Segoe UI', Tahoma, sans-serif");
            let width = self
                .ctx
                .measure_text(&peer.name)
                .map(|m| m.width())
                .unwrap_or(60.0)
                + 12.0;
            let bx = x + 14.0;
            let by = y + 19.0;
            self.ctx.set_fill_style_str(&peer.color);
            self.ctx.fill_rect(bx, by, width, 17.0);
            self.ctx.set_fill_style_str("#ffffff");
            let _ = self.ctx.fill_text(&peer.name, bx + 6.0, by + 12.5);
        }
        self.ctx.set_fill_style_str("#1e1e1e");
        self.ctx.set_stroke_style_str("#1e1e1e");
    }

    pub fn sync_ui(&mut self) {
        let doc = self.document.clone();
        let active_tool = self.tool_data();
        for el in query_all(&doc, ".tool[data-tool]") {
            let mut active = el.get_attribute("data-tool").as_deref() == Some(active_tool);
            if active {
                if let Some(width) = el.get_attribute("data-width") {
                    active = width
                        .parse::<f64>()
                        .map(|v| (v - self.style.width).abs() < 0.01)
                        .unwrap_or(false);
                }
            }
            set_active(&el, active);
        }

        let style = &self.style;
        for el in query_all(&doc, "[data-stroke]") {
            let active = el.get_attribute("data-stroke").as_deref() == Some(style.stroke.as_str());
            set_active(&el, active);
        }
        for el in query_all(&doc, "[data-bg]") {
            let active = el.get_attribute("data-bg").as_deref() == Some(style.bg.as_str());
            set_active(&el, active);
        }
        let fill_name = match style.fill {
            Fill::Hachure => "hachure",
            Fill::Cross => "cross",
            Fill::Solid => "solid",
        };
        for el in query_all(&doc, "[data-fill]") {
            let active = el.get_attribute("data-fill").as_deref() == Some(fill_name);
            set_active(&el, active);
        }
        for el in query_all(&doc, "[data-width]") {
            if el.has_attribute("data-tool") {
                continue;
            }
            let active = el
                .get_attribute("data-width")
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| (v - style.width).abs() < 0.01)
                .unwrap_or(false);
            set_active(&el, active);
        }
        for el in query_all(&doc, "[data-font]") {
            let active = el
                .get_attribute("data-font")
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| (v - style.font).abs() < 0.01)
                .unwrap_or(false);
            set_active(&el, active);
        }

        if let Some(panel) = doc.get_element_by_id("panel") {
            let show = !self.selected.is_empty()
                || matches!(
                    self.tool,
                    Tool::Rectangle
                        | Tool::Diamond
                        | Tool::Ellipse
                        | Tool::Arrow
                        | Tool::Line
                        | Tool::Draw
                        | Tool::Text
                );
            set_active(&panel, show);
        }

        if let Some(label) = doc.get_element_by_id("zoom-label") {
            label.set_text_content(Some(&format!("{:.0}%", self.zoom * 100.0)));
        }
        if let Some(btn) = doc.get_element_by_id("undo") {
            toggle_disabled(&btn, !self.history.can_undo());
        }
        if let Some(btn) = doc.get_element_by_id("redo") {
            toggle_disabled(&btn, !self.history.can_redo());
        }
        if let Some(btn) = doc.get_element_by_id("lock") {
            set_active(&btn, self.locked);
        }
        if let Some(btn) = doc.get_element_by_id("collab") {
            set_active(&btn, self.collab.connected);
        }
        if let Some(chip) = doc.get_element_by_id("live") {
            if self.collab.connected {
                let _ = chip.class_list().remove_1("hidden");
                let count = self.collab.peers.len() + 1;
                if let Some(label) = doc.get_element_by_id("live-label") {
                    label.set_text_content(Some(&format!("Live · {count}")));
                }
            } else {
                let _ = chip.class_list().add_1("hidden");
            }
        }
        if let Some(chip) = doc
            .get_element_by_id("color-1")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        {
            let _ = chip.style().set_property("background", &self.style.stroke);
        }
        if let Some(chip) = doc
            .get_element_by_id("color-2")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        {
            let bg = if self.style.bg == "transparent" {
                "#ffffff"
            } else {
                &self.style.bg
            };
            let _ = chip.style().set_property("background", bg);
        }
        let has_selection = !self.selected.is_empty();
        for id in ["ctx-cut", "ctx-copy", "ctx-duplicate", "ctx-delete"] {
            if let Some(btn) = doc.get_element_by_id(id) {
                toggle_disabled(&btn, !has_selection);
            }
        }
        if let Some(btn) = doc.get_element_by_id("ctx-paste") {
            toggle_disabled(&btn, self.clipboard.is_empty());
        }
        if let Some(btn) = doc.get_element_by_id("ctx-select-all") {
            toggle_disabled(&btn, self.elements.is_empty());
        }

        let cursor = if self.space || matches!(self.drag, Drag::Pan { .. }) {
            "grabbing"
        } else if self.tool == Tool::Hand {
            "grab"
        } else if self.tool == Tool::Text {
            "text"
        } else if self.tool == Tool::Select {
            "default"
        } else {
            "crosshair"
        };
        let _ = self.canvas.style().set_property("cursor", cursor);
    }

    fn seed_for(&self, id: u64) -> u32 {
        (id as u32)
            .wrapping_mul(2_654_435_761)
            .wrapping_add(0x9E37_79B9)
    }

    fn alloc_id(&mut self) -> u64 {
        if self.collab.session_base > 0 {
            loop {
                self.collab.session_seq += 1;
                let id = self.collab.session_base + self.collab.session_seq;
                if !self.elements.iter().any(|e| e.id == id) {
                    self.next_id = self.next_id.max(id);
                    return id;
                }
            }
        }
        self.next_id += 1;
        self.next_id
    }

    fn new_element(&mut self, kind: Kind, x: f64, y: f64) -> Element {
        let id = self.alloc_id();
        let seed = self.seed_for(id);
        Element::new(id, kind, x, y, self.style.clone(), seed)
    }

    fn to_world(&self, p: Pt) -> Pt {
        Pt::new(
            (p.x - self.pan.x) / self.zoom,
            (p.y - self.pan.y) / self.zoom,
        )
    }

    fn pointer_pos(&self, ev: &PointerEvent) -> Pt {
        let rect = self.canvas.get_bounding_client_rect();
        Pt::new(
            ev.client_x() as f64 - rect.left(),
            ev.client_y() as f64 - rect.top(),
        )
    }

    fn on_context_menu(&mut self, ev: &MouseEvent) {
        ev.prevent_default();
        let rect = self.canvas.get_bounding_client_rect();
        let p = Pt::new(
            ev.client_x() as f64 - rect.left(),
            ev.client_y() as f64 - rect.top(),
        );
        match self.hit_test(self.to_world(p)) {
            Some(id) if !self.selected.contains(&id) => self.selected = vec![id],
            Some(_) => {}
            None => self.selected.clear(),
        }
        self.sync_ui();
        self.draw();
        self.close_menus();
        let Some(menu) = self
            .document
            .get_element_by_id("context-menu")
            .and_then(|e| e.dyn_into::<HtmlElement>().ok())
        else {
            return;
        };
        let vw = self
            .window
            .inner_width()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(800.0);
        let vh = self
            .window
            .inner_height()
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(600.0);
        let left = (ev.client_x() as f64).min(vw - 212.0).max(4.0);
        let top = (ev.client_y() as f64).min(vh - 224.0).max(4.0);
        let _ = menu.class_list().add_1("open");
        let _ = menu.style().set_property("left", &format!("{left:.0}px"));
        let _ = menu.style().set_property("top", &format!("{top:.0}px"));
    }

    fn selection_bounds(&self) -> Option<Rect> {
        let mut iter = self
            .selected
            .iter()
            .filter_map(|id| self.elements.iter().find(|e| e.id == *id))
            .map(|e| e.bounds());
        let first = iter.next()?;
        Some(iter.fold(first, |acc, r| acc.union(&r)))
    }

    fn marquee_rect(&self) -> Option<Rect> {
        match &self.drag {
            Drag::Marquee { start, cur, .. } => Some(Rect::from_points(*start, *cur)),
            _ => None,
        }
    }

    fn hit_test(&self, p: Pt) -> Option<u64> {
        let tol = 7.0 / self.zoom;
        for e in self.elements.iter().rev() {
            match e.kind {
                Kind::Line | Kind::Arrow | Kind::Draw => {
                    if e.points.len() >= 2 {
                        let mut hit = false;
                        for pair in e.points.windows(2) {
                            if dist_to_segment(p, pair[0], pair[1]) <= tol {
                                hit = true;
                                break;
                            }
                        }
                        if !hit && e.points.len() == 1 {
                            let d = ((p.x - e.points[0].x).powi(2) + (p.y - e.points[0].y).powi(2))
                                .sqrt();
                            hit = d <= tol;
                        }
                        if hit {
                            return Some(e.id);
                        }
                    }
                }
                _ => {
                    if e.bounds().inflate(tol).contains(p) {
                        return Some(e.id);
                    }
                }
            }
        }
        None
    }

    fn hit_handle(&self, p: Pt, bounds: Rect) -> Option<usize> {
        render::handle_rects(bounds, self.zoom)
            .iter()
            .position(|r| r.inflate(3.0 / self.zoom).contains(p))
    }

    fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.sync_ui();
    }

    fn close_menus(&self) {
        for id in ["main-menu", "more-menu", "context-menu"] {
            if let Some(el) = self.document.get_element_by_id(id) {
                let _ = el.class_list().remove_1("open");
            }
        }
    }

    fn hide_hint(&self) {
        if let Some(el) = self.document.get_element_by_id("hint") {
            let _ = el.class_list().add_1("hidden");
        }
    }

    pub fn on_pointer_down(&mut self, ev: &PointerEvent) {
        self.close_menus();
        self.hide_hint();
        if ev.button() == 0 {
            ev.prevent_default();
        }
        let screen = self.pointer_pos(ev);
        self.pointer = screen;
        let world = self.to_world(screen);

        let middle = ev.button() == 1;
        if middle || self.space || self.tool == Tool::Hand {
            self.drag = Drag::Pan {
                start: screen,
                pan0: self.pan,
            };
            self.sync_ui();
            return;
        }
        if ev.button() != 0 {
            return;
        }

        match self.tool {
            Tool::Select => {
                if ev.detail() >= 2 {
                    if let Some(id) = self.hit_test(world) {
                        if let Some(e) = self.elements.iter().find(|e| e.id == id) {
                            if e.kind == Kind::Text {
                                let before = self.elements.clone();
                                self.selected = vec![id];
                                self.open_text_editor(id, before, false);
                                return;
                            }
                        }
                    }
                }

                if let Some(bounds) = self.selection_bounds() {
                    if let Some(handle) = self.hit_handle(world, bounds) {
                        let before = self.elements.clone();
                        self.drag = Drag::Resize {
                            handle,
                            start: world,
                            before,
                            bounds,
                        };
                        self.sync_ui();
                        return;
                    }
                }

                if let Some(id) = self.hit_test(world) {
                    if ev.shift_key() {
                        if let Some(pos) = self.selected.iter().position(|x| *x == id) {
                            self.selected.remove(pos);
                        } else {
                            self.selected.push(id);
                        }
                    } else if !self.selected.contains(&id) {
                        self.selected = vec![id];
                    }
                    let before = self.elements.clone();
                    self.drag = Drag::Move {
                        start: world,
                        before,
                    };
                    self.sync_ui();
                    self.draw();
                } else {
                    if !ev.shift_key() {
                        self.selected.clear();
                    }
                    self.drag = Drag::Marquee {
                        start: world,
                        cur: world,
                        additive: ev.shift_key(),
                    };
                    self.sync_ui();
                    self.draw();
                }
            }
            Tool::Rectangle | Tool::Diamond | Tool::Ellipse | Tool::Line | Tool::Arrow => {
                let before = self.elements.clone();
                let kind = match self.tool {
                    Tool::Rectangle => Kind::Rectangle,
                    Tool::Diamond => Kind::Diamond,
                    Tool::Ellipse => Kind::Ellipse,
                    Tool::Line => Kind::Line,
                    _ => Kind::Arrow,
                };
                let mut el = self.new_element(kind, world.x, world.y);
                if el.is_linear() {
                    el.points = vec![world, world];
                }
                let id = el.id;
                self.elements.push(el);
                self.selected = vec![id];
                self.drag = Drag::Create {
                    id,
                    start: world,
                    before,
                };
                self.sync_ui();
                self.draw();
            }
            Tool::Draw => {
                let before = self.elements.clone();
                let mut el = self.new_element(Kind::Draw, world.x, world.y);
                el.points = vec![world];
                let id = el.id;
                self.elements.push(el);
                self.selected = vec![id];
                self.drag = Drag::Draw { id, before };
                self.sync_ui();
                self.draw();
            }
            Tool::Text => {
                let before = self.elements.clone();
                let mut el = self.new_element(Kind::Text, world.x, world.y);
                el.w = 120.0;
                el.h = self.style.font * 1.25;
                let id = el.id;
                self.elements.push(el);
                self.selected = vec![id];
                self.open_text_editor(id, before, true);
                self.sync_ui();
            }
            Tool::Eraser => {
                let before = self.elements.clone();
                let mut removed = false;
                if let Some(id) = self.hit_test(world) {
                    self.elements.retain(|e| e.id != id);
                    removed = true;
                }
                if removed {
                    self.selected.clear();
                }
                self.drag = Drag::Erase { before };
                self.draw();
                self.sync_ui();
            }
            Tool::Image => {
                self.pending_image_pos = Some(world);
                self.open_file_picker("file-image");
            }
            Tool::Hand => {}
        }
    }

    pub fn on_pointer_move(&mut self, ev: &PointerEvent) {
        let screen = self.pointer_pos(ev);
        self.pointer = screen;
        let world = self.to_world(screen);
        if screen.x >= 0.0 && screen.y >= 0.0 && screen.x <= self.css_w && screen.y <= self.css_h {
            self.send_cursor(Some(world));
        }
        let mut changed = false;

        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => return,
            Drag::Pan { start, pan0 } => {
                self.pan = Pt::new(pan0.x + (screen.x - start.x), pan0.y + (screen.y - start.y));
                self.drag = Drag::Pan { start, pan0 };
                changed = true;
            }
            Drag::Move { start, before } => {
                let dx = world.x - start.x;
                let dy = world.y - start.y;
                let selected = self.selected.clone();
                for el in self.elements.iter_mut() {
                    if selected.contains(&el.id) {
                        if let Some(orig) = before.iter().find(|b| b.id == el.id) {
                            *el = orig.clone();
                            el.translate(dx, dy);
                        }
                    }
                }
                self.drag = Drag::Move { start, before };
                changed = true;
            }
            Drag::Resize {
                handle,
                start,
                before,
                bounds,
            } => {
                let nb = resize_rect(bounds, world, handle);
                if nb.w > 1e-6 && nb.h > 1e-6 {
                    let sx = nb.w / bounds.w.max(1e-6);
                    let sy = nb.h / bounds.h.max(1e-6);
                    for el in self.elements.iter_mut() {
                        if !self.selected.contains(&el.id) {
                            continue;
                        }
                        if let Some(orig) = before.iter().find(|b| b.id == el.id) {
                            let mut next = orig.clone();
                            next.x = nb.x + (orig.x - bounds.x) * sx;
                            next.y = nb.y + (orig.y - bounds.y) * sy;
                            next.w = orig.w * sx;
                            next.h = orig.h * sy;
                            if orig.is_linear() {
                                for (i, p) in next.points.iter_mut().enumerate() {
                                    let op = orig.points.get(i).copied().unwrap_or(*p);
                                    p.x = nb.x + (op.x - bounds.x) * sx;
                                    p.y = nb.y + (op.y - bounds.y) * sy;
                                }
                            }
                            if orig.kind == Kind::Text {
                                next.style.font = (orig.style.font * sx.min(sy)).max(6.0);
                            }
                            *el = next;
                        }
                    }
                }
                self.drag = Drag::Resize {
                    handle,
                    start,
                    before,
                    bounds,
                };
                changed = true;
            }
            Drag::Create { id, start, before } => {
                if let Some(el) = self.elements.iter_mut().find(|e| e.id == id) {
                    if el.is_linear() {
                        let mut end = world;
                        if ev.shift_key() {
                            end = snap_angle(start, end);
                        }
                        el.points = vec![start, end];
                        let b = el.bounds();
                        el.x = b.x;
                        el.y = b.y;
                        el.w = b.w;
                        el.h = b.h;
                    } else {
                        let mut end = world;
                        if ev.shift_key() {
                            let dx = (end.x - start.x).abs();
                            let dy = (end.y - start.y).abs();
                            let side = dx.max(dy);
                            end.x = start.x + side * if end.x >= start.x { 1.0 } else { -1.0 };
                            end.y = start.y + side * if end.y >= start.y { 1.0 } else { -1.0 };
                        }
                        let r = Rect::from_points(start, end);
                        el.set_from_rect(r);
                    }
                }
                self.drag = Drag::Create { id, start, before };
                changed = true;
            }
            Drag::Draw { id, before } => {
                if let Some(el) = self.elements.iter_mut().find(|e| e.id == id) {
                    let min_step = 1.2 / self.zoom;
                    let last = el.points.last().copied().unwrap_or(world);
                    let d = ((world.x - last.x).powi(2) + (world.y - last.y).powi(2)).sqrt();
                    if d >= min_step {
                        el.points.push(world);
                        let b = el.bounds();
                        el.x = b.x;
                        el.y = b.y;
                        el.w = b.w;
                        el.h = b.h;
                    }
                }
                self.drag = Drag::Draw { id, before };
                changed = true;
            }
            Drag::Marquee {
                start,
                cur: _,
                additive,
            } => {
                self.drag = Drag::Marquee {
                    start,
                    cur: world,
                    additive,
                };
                self.draw();
                return;
            }
            Drag::Erase { before } => {
                if let Some(id) = self.hit_test(world) {
                    self.elements.retain(|e| e.id != id);
                    self.selected.clear();
                    changed = true;
                }
                self.drag = Drag::Erase { before };
            }
        }

        if changed {
            self.collab_publish(false);
            self.draw();
            self.sync_ui();
        }
    }

    pub fn on_pointer_up(&mut self, _ev: &PointerEvent) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Create { id, before, .. } => {
                let created = self.elements.iter().find(|e| e.id == id).cloned();
                if let Some(el) = created {
                    let too_small = if el.is_linear() {
                        let b = el.bounds();
                        el.points.len() < 2 || b.w.hypot(b.h) < 3.0
                    } else {
                        el.w < 3.0 && el.h < 3.0
                    };
                    if too_small {
                        self.elements.retain(|e| e.id != id);
                        self.selected.clear();
                    }
                }
                if self.elements != before {
                    self.history.push(before);
                    self.save();
                }
                if !self.locked {
                    self.tool = Tool::Select;
                }
            }
            Drag::Move { before, .. }
            | Drag::Resize { before, .. }
            | Drag::Erase { before }
            | Drag::Draw { before, .. } => {
                if self.elements != before {
                    self.history.push(before);
                    self.save();
                }
            }
            Drag::Marquee {
                start,
                cur,
                additive,
            } => {
                let marquee = Rect::from_points(start, cur);
                if marquee.w > 2.0 || marquee.h > 2.0 {
                    let mut hits: Vec<u64> = self
                        .elements
                        .iter()
                        .filter(|e| e.bounds().intersects(&marquee))
                        .map(|e| e.id)
                        .collect();
                    hits.sort_unstable();
                    if additive {
                        let mut merged = self.selected.clone();
                        merged.append(&mut hits);
                        merged.sort_unstable();
                        merged.dedup();
                        self.selected = merged;
                    } else {
                        self.selected = hits;
                    }
                }
            }
            Drag::Pan { .. } | Drag::None => {}
        }

        self.collab_publish(true);
        self.draw();
        self.sync_ui();
    }

    pub fn on_wheel(&mut self, ev: &WheelEvent) {
        ev.prevent_default();
        let rect = self.canvas.get_bounding_client_rect();
        let sx = ev.client_x() as f64 - rect.left();
        let sy = ev.client_y() as f64 - rect.top();

        if ev.ctrl_key() || ev.meta_key() {
            let factor = (-ev.delta_y() * 0.0018).exp();
            let new_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            let ratio = new_zoom / self.zoom;
            self.pan.x = sx - (sx - self.pan.x) * ratio;
            self.pan.y = sy - (sy - self.pan.y) * ratio;
            self.zoom = new_zoom;
        } else {
            self.pan.x -= ev.delta_x();
            self.pan.y -= ev.delta_y();
        }
        self.draw();
        self.sync_ui();
    }

    pub fn on_key_down(&mut self, ev: &KeyboardEvent) {
        if self.is_typing() {
            return;
        }
        let key = ev.key();
        let lower = key.to_lowercase();
        let meta = ev.ctrl_key() || ev.meta_key();

        if key == " " {
            self.space = true;
            ev.prevent_default();
            self.sync_ui();
            return;
        }

        if meta {
            match lower.as_str() {
                "z" => {
                    ev.prevent_default();
                    if ev.shift_key() {
                        self.redo();
                    } else {
                        self.undo();
                    }
                }
                "y" => {
                    ev.prevent_default();
                    self.redo();
                }
                "a" => {
                    ev.prevent_default();
                    self.selected = self.elements.iter().map(|e| e.id).collect();
                    self.sync_ui();
                    self.draw();
                }
                "c" => self.copy(),
                "v" => self.paste(),
                "d" => {
                    ev.prevent_default();
                    self.duplicate();
                }
                "s" => {
                    ev.prevent_default();
                    self.export_json();
                }
                _ => {}
            }
            return;
        }

        match lower.as_str() {
            "v" => self.set_tool(Tool::Select),
            "h" => self.set_tool(Tool::Hand),
            "r" => self.set_tool(Tool::Rectangle),
            "d" => self.set_tool(Tool::Diamond),
            "o" => self.set_tool(Tool::Ellipse),
            "a" => self.set_tool(Tool::Arrow),
            "l" => self.set_tool(Tool::Line),
            "p" => self.set_tool(Tool::Draw),
            "t" => self.set_tool(Tool::Text),
            "e" => self.set_tool(Tool::Eraser),
            "i" => {
                self.set_tool(Tool::Image);
                self.open_file_picker("file-image");
            }
            "delete" | "backspace" => {
                ev.prevent_default();
                self.delete_selected();
            }
            "escape" => {
                self.commit_text(true);
                self.selected.clear();
                self.set_tool(Tool::Select);
                self.close_menus();
                self.draw();
            }
            "arrowleft" => self.nudge(-1.0, 0.0, ev.shift_key()),
            "arrowright" => self.nudge(1.0, 0.0, ev.shift_key()),
            "arrowup" => self.nudge(0.0, -1.0, ev.shift_key()),
            "arrowdown" => self.nudge(0.0, 1.0, ev.shift_key()),
            "0" => self.reset_zoom(),
            _ => {}
        }
    }

    pub fn on_key_up(&mut self, ev: &KeyboardEvent) {
        if ev.key() == " " {
            self.space = false;
            self.sync_ui();
        }
    }

    fn is_typing(&self) -> bool {
        self.document
            .active_element()
            .map(|el| el.tag_name())
            .map(|tag| tag == "TEXTAREA" || tag == "INPUT")
            .unwrap_or(false)
    }

    fn nudge(&mut self, dx: f64, dy: f64, big: bool) {
        if self.selected.is_empty() {
            return;
        }
        let before = self.elements.clone();
        let step = if big { 10.0 } else { 1.0 };
        for el in self.elements.iter_mut() {
            if self.selected.contains(&el.id) {
                el.translate(dx * step, dy * step);
            }
        }
        if self.elements != before {
            self.history.push(before);
            self.save();
        }
        self.draw();
    }

    fn delete_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let before = self.elements.clone();
        self.elements.retain(|e| !self.selected.contains(&e.id));
        self.selected.clear();
        self.history.push(before);
        self.save();
        self.sync_ui();
        self.draw();
    }

    fn copy(&mut self) {
        self.clipboard = self
            .elements
            .iter()
            .filter(|e| self.selected.contains(&e.id))
            .cloned()
            .collect();
        if !self.clipboard.is_empty() {
            self.toast("Copied");
        }
    }

    fn paste(&mut self) {
        if self.clipboard.is_empty() {
            return;
        }
        let before = self.elements.clone();
        let mut new_ids = Vec::new();
        let clips = self.clipboard.clone();
        for mut el in clips {
            el.id = self.alloc_id();
            el.seed = self.seed_for(el.id);
            el.translate(16.0, 16.0);
            new_ids.push(el.id);
            self.elements.push(el);
        }
        self.selected = new_ids;
        self.history.push(before);
        self.save();
        self.sync_ui();
        self.draw();
    }

    fn duplicate(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        self.copy();
        self.paste();
    }

    fn undo(&mut self) {
        let current = self.elements.clone();
        if let Some(prev) = self.history.undo(current) {
            self.elements = prev;
            self.selected.clear();
            self.text_edit = None;
            self.save();
            self.sync_ui();
            self.draw();
        }
    }

    fn redo(&mut self) {
        let current = self.elements.clone();
        if let Some(next) = self.history.redo(current) {
            self.elements = next;
            self.selected.clear();
            self.text_edit = None;
            self.save();
            self.sync_ui();
            self.draw();
        }
    }

    pub fn set_style_value(&mut self, key: &str, value: &str) {
        let before = self.elements.clone();
        let apply = |s: &mut Style| match key {
            "stroke" => s.stroke = value.to_string(),
            "bg" => s.bg = value.to_string(),
            "fill" => {
                s.fill = match value {
                    "cross" => Fill::Cross,
                    "solid" => Fill::Solid,
                    _ => Fill::Hachure,
                }
            }
            "width" => {
                if let Ok(v) = value.parse::<f64>() {
                    s.width = v;
                }
            }
            "font" => {
                if let Ok(v) = value.parse::<f64>() {
                    s.font = v;
                }
            }
            _ => {}
        };
        apply(&mut self.style);
        let ids = self.selected.clone();
        for el in self.elements.iter_mut() {
            if ids.contains(&el.id) {
                apply(&mut el.style);
            }
        }
        if self.elements != before {
            self.history.push(before);
            self.save();
        }
        self.sync_ui();
        self.draw();
    }

    fn open_text_editor(&mut self, id: u64, before: Vec<Element>, is_new: bool) {
        let (text, x, y, font, color) = match self.elements.iter().find(|e| e.id == id) {
            Some(e) => (
                e.text.clone(),
                e.x,
                e.y,
                e.style.font,
                e.style.stroke.clone(),
            ),
            None => return,
        };
        let Some(editor) = self.document.get_element_by_id("text-editor") else {
            return;
        };
        let Ok(editor) = editor.dyn_into::<HtmlTextAreaElement>() else {
            return;
        };
        self.text_edit = Some(id);
        self.text_before = Some(before);
        self.text_new = is_new;
        editor.set_value(&text);
        let _ = editor.style().set_property("display", "block");
        let rect = self.canvas.get_bounding_client_rect();
        let _ = editor.style().set_property(
            "left",
            &format!("{:.1}px", rect.left() + x * self.zoom + self.pan.x),
        );
        let _ = editor.style().set_property(
            "top",
            &format!("{:.1}px", rect.top() + y * self.zoom + self.pan.y),
        );
        let _ = editor
            .style()
            .set_property("font-size", &format!("{:.1}px", font * self.zoom));
        let _ = editor
            .style()
            .set_property("line-height", &format!("{:.2}", 1.25));
        let _ = editor.style().set_property("color", &color);
        let width = (140.0 * self.zoom).max(80.0);
        let _ = editor
            .style()
            .set_property("width", &format!("{width:.0}px"));
        let _ = editor
            .style()
            .set_property("height", &format!("{:.0}px", font * 1.35 * self.zoom));
        let _ = editor.focus();
        let len = editor.value().len() as u32;
        let _ = editor.set_selection_range(len, len);
    }

    pub fn commit_text(&mut self, cancel: bool) {
        let Some(id) = self.text_edit.take() else {
            return;
        };
        let editor = self
            .document
            .get_element_by_id("text-editor")
            .and_then(|e| e.dyn_into::<HtmlTextAreaElement>().ok());
        let value = editor.as_ref().map(|e| e.value()).unwrap_or_default();
        if let Some(editor) = &editor {
            let _ = editor.style().set_property("display", "none");
            let _ = editor.blur();
        }
        let Some(before) = self.text_before.take() else {
            return;
        };

        if cancel {
            if self.text_new {
                self.elements.retain(|e| e.id != id);
            } else {
                self.elements = before;
            }
            self.selected.clear();
        } else {
            let text = value.trim_end().to_string();
            if text.is_empty() {
                self.elements.retain(|e| e.id != id);
                self.selected.clear();
            } else {
                let font = self
                    .elements
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.style.font)
                    .unwrap_or(self.style.font);
                let (w, h) = self.measure_text(&text, font);
                if let Some(el) = self.elements.iter_mut().find(|e| e.id == id) {
                    el.text = text;
                    el.w = w;
                    el.h = h;
                }
            }
            if self.elements != before {
                self.history.push(before);
                self.save();
            }
            self.selected = vec![id];
        }
        if !self.locked {
            self.tool = Tool::Select;
        }
        self.sync_ui();
        self.draw();
    }

    fn measure_text(&self, text: &str, font: f64) -> (f64, f64) {
        self.ctx.set_font(&format!("{font}px {FONT}"));
        let mut width: f64 = font * 0.5;
        let mut lines = 0usize;
        for line in text.split('\n') {
            lines += 1;
            if let Ok(m) = self.ctx.measure_text(line) {
                width = width.max(m.width());
            }
        }
        (width, (lines.max(1) as f64) * font * 1.25)
    }

    fn zoom_by(&mut self, factor: f64) {
        let cx = self.css_w / 2.0;
        let cy = self.css_h / 2.0;
        let new_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let ratio = new_zoom / self.zoom;
        self.pan.x = cx - (cx - self.pan.x) * ratio;
        self.pan.y = cy - (cy - self.pan.y) * ratio;
        self.zoom = new_zoom;
        self.sync_ui();
        self.draw();
    }

    fn reset_zoom(&mut self) {
        self.zoom = 1.0;
        self.sync_ui();
        self.draw();
    }

    fn fit_content(&mut self) {
        if self.elements.is_empty() {
            return;
        }
        let mut bounds = self.elements[0].bounds();
        for e in &self.elements[1..] {
            bounds = bounds.union(&e.bounds());
        }
        if bounds.w < 1.0 {
            bounds.w = 1.0;
        }
        if bounds.h < 1.0 {
            bounds.h = 1.0;
        }
        let zx = (self.css_w - 140.0).max(80.0) / bounds.w;
        let zy = (self.css_h - 140.0).max(80.0) / bounds.h;
        self.zoom = zx.min(zy).clamp(0.15, 1.2);
        let c = bounds.center();
        self.pan = Pt::new(
            self.css_w / 2.0 - c.x * self.zoom,
            self.css_h / 2.0 - c.y * self.zoom,
        );
        self.sync_ui();
        self.draw();
    }

    fn room_from_url(&self) -> Option<String> {
        let search = self.window.location().search().ok()?;
        let query = search.strip_prefix('?').unwrap_or(&search);
        for pair in query.split('&') {
            let Some(value) = pair.strip_prefix("room=") else {
                continue;
            };
            let room: String = value
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect();
            if !room.is_empty() {
                return Some(room);
            }
        }
        None
    }

    fn collab_link(&self) -> String {
        let location = self.window.location();
        format!(
            "{}{}?room={}",
            location.origin().unwrap_or_default(),
            location.pathname().unwrap_or_default(),
            self.collab.room
        )
    }

    fn apply_room_url(&self, room: &str) {
        let location = self.window.location();
        let url = format!(
            "{}?room={room}",
            location.pathname().unwrap_or_else(|_| "/".to_string())
        );
        if let Ok(history) = self.window.history() {
            let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url));
        }
    }

    pub fn start_if_room(&mut self) {
        if let Some(room) = self.room_from_url() {
            self.collab.room = room;
            self.connect_collab();
        }
    }

    fn start_collab(&mut self) {
        if self.collab.ws.is_some() {
            let url = self.collab_link();
            let _ = self.window.navigator().clipboard().write_text(&url);
            if self.collab.connected {
                self.toast("Live invite link copied");
            } else {
                self.toast("Connecting — invite link copied");
            }
            return;
        }
        self.commit_text(false);
        let room = self
            .room_from_url()
            .unwrap_or_else(|| format!("room-{}", random_token(10)));
        self.apply_room_url(&room);
        self.collab.room = room;
        self.collab.retry_delay = 1000.0;
        self.connect_collab();
        if let Some(ws) = &self.collab.ws {
            let _ = ws;
            let url = self.collab_link();
            let _ = self.window.navigator().clipboard().write_text(&url);
            self.toast("Live link copied — waiting for collaborators");
        } else {
            self.toast("Could not start live collaboration");
        }
    }

    fn connect_collab(&mut self) {
        if self.collab.room.is_empty() {
            return;
        }
        self.collab.generation = self.collab.generation.wrapping_add(1);
        let generation = self.collab.generation;
        if let Some(ws) = self.collab.ws.take() {
            let _ = ws.close();
        }
        self.collab.connected = false;
        self.collab.ready = false;
        self.collab.peers.clear();
        self.collab.last_pub.clear();
        self.collab.last_pub_at = 0.0;
        self.collab.last_cursor = None;

        let location = self.window.location();
        let proto = if location.protocol().unwrap_or_default() == "https:" {
            "wss"
        } else {
            "ws"
        };
        let host = location
            .host()
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let url = format!("{proto}://{host}/ws?room={}", self.collab.room);
        let Ok(ws) = WebSocket::new(&url) else {
            self.toast("Could not open live connection — retrying");
            self.schedule_reconnect(generation);
            return;
        };
        self.collab.ws = Some(ws.clone());

        let weak = self.self_ref.clone();
        on::<Event>(&ws, "open", move |_| {
            if let Some(app) = weak.upgrade() {
                app.borrow_mut().on_ws_open(generation);
            }
        });
        let weak = self.self_ref.clone();
        on::<MessageEvent>(&ws, "message", move |ev| {
            let Some(data) = ev.data().as_string() else {
                return;
            };
            if let Some(app) = weak.upgrade() {
                app.borrow_mut().on_ws_message(generation, &data);
            }
        });
        let weak = self.self_ref.clone();
        on::<Event>(&ws, "close", move |_| {
            if let Some(app) = weak.upgrade() {
                app.borrow_mut().on_ws_close(generation);
            }
        });
        self.sync_ui();
    }

    fn on_ws_open(&mut self, generation: u64) {
        if generation != self.collab.generation {
            return;
        }
        let hello = serde_json::json!({
            "t": "hello",
            "name": self.collab.name,
            "color": self.collab.color,
        })
        .to_string();
        if let Some(ws) = &self.collab.ws {
            let _ = ws.send_with_str(&hello);
        }
    }

    fn on_ws_message(&mut self, generation: u64, data: &str) {
        if generation != self.collab.generation {
            return;
        }
        let Ok(message) = serde_json::from_str::<ServerMsg>(data) else {
            return;
        };
        match message {
            ServerMsg::Init {
                client_id,
                elements,
                peers,
            } => self.on_ws_init(client_id, elements, peers),
            ServerMsg::Peers { peers } => self.on_ws_peers(peers),
            ServerMsg::Patch { upsert, delete } => self.on_ws_patch(upsert, delete),
            ServerMsg::Cursor { id, x, y } => {
                if id == self.collab.client_id {
                    return;
                }
                let cursor = match (x, y) {
                    (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some(Pt::new(x, y)),
                    _ => None,
                };
                let Some(peer) = self.collab.peers.get_mut(&id) else {
                    return;
                };
                if peer.cursor == cursor {
                    return;
                }
                peer.cursor = cursor;
                self.draw();
            }
            ServerMsg::Bye { id } => {
                if self.collab.peers.remove(&id).is_some() {
                    self.sync_ui();
                    self.draw();
                }
            }
            ServerMsg::Other => {}
        }
    }

    fn on_ws_init(&mut self, client_id: u64, elements: Vec<Element>, peers: Vec<PeerInfo>) {
        self.collab.client_id = client_id;
        self.collab.session_base = client_id.saturating_mul(ID_PREFIX);
        self.collab.session_seq = 0;
        self.collab.retry_delay = 1000.0;
        self.collab.connected = true;
        self.collab.ready = true;
        self.collab.peers = peers
            .into_iter()
            .filter(|peer| peer.id != client_id && !peer.name.is_empty())
            .map(|peer| {
                (
                    peer.id,
                    Peer {
                        name: peer.name,
                        color: peer.color,
                        cursor: peer.cursor,
                    },
                )
            })
            .collect();
        self.collab.last_pub.clear();
        self.selected.clear();
        self.history.clear();

        if elements.is_empty() {
            let base = self.collab.session_base;
            for (index, element) in self.elements.iter_mut().enumerate() {
                element.id = base + index as u64 + 1;
            }
            self.collab.session_seq = self.elements.len() as u64;
            self.next_id = self
                .elements
                .iter()
                .map(|element| element.id)
                .max()
                .unwrap_or(base);
            self.collab_publish(true);
            self.save();
        } else {
            self.elements = elements;
            for element in &self.elements {
                self.collab.last_pub.insert(element.id, element.clone());
            }
            self.next_id = self
                .elements
                .iter()
                .map(|element| element.id)
                .max()
                .unwrap_or(self.collab.session_base);
            self.save();
        }
        self.sync_ui();
        self.draw();
        let others = self.collab.peers.len();
        if others == 0 {
            self.toast("Live — copy the invite link to add someone");
        } else {
            self.toast(&format!(
                "Live — {others} other{} connected",
                if others == 1 { "" } else { "s" }
            ));
        }
    }

    fn on_ws_peers(&mut self, peers: Vec<PeerInfo>) {
        let me = self.collab.client_id;
        self.collab.peers = peers
            .into_iter()
            .filter(|peer| peer.id != me && !peer.name.is_empty())
            .map(|peer| {
                (
                    peer.id,
                    Peer {
                        name: peer.name,
                        color: peer.color,
                        cursor: peer.cursor,
                    },
                )
            })
            .collect();
        self.sync_ui();
        self.draw();
    }

    fn on_ws_patch(&mut self, upsert: Vec<Element>, delete: Vec<u64>) {
        let mut changed = false;
        for element in upsert {
            self.collab.last_pub.insert(element.id, element.clone());
            if let Some(existing) = self
                .elements
                .iter_mut()
                .find(|existing| existing.id == element.id)
            {
                if *existing != element {
                    *existing = element;
                    changed = true;
                }
            } else {
                self.elements.push(element);
                changed = true;
            }
        }
        if !delete.is_empty() {
            let before = self.elements.len();
            self.elements
                .retain(|element| !delete.contains(&element.id));
            if self.elements.len() != before {
                changed = true;
            }
            self.selected.retain(|id| !delete.contains(id));
            for id in &delete {
                self.collab.last_pub.remove(id);
            }
            if self
                .text_edit
                .map(|id| delete.contains(&id))
                .unwrap_or(false)
            {
                self.text_edit = None;
                self.text_before = None;
                if let Some(editor) = self
                    .document
                    .get_element_by_id("text-editor")
                    .and_then(|e| e.dyn_into::<HtmlTextAreaElement>().ok())
                {
                    let _ = editor.style().set_property("display", "none");
                }
            }
        }
        if !changed {
            return;
        }
        self.next_id = self.next_id.max(
            self.elements
                .iter()
                .map(|element| element.id)
                .max()
                .unwrap_or(0),
        );
        self.save();
        self.sync_ui();
        self.draw();
    }

    fn on_ws_close(&mut self, generation: u64) {
        if generation != self.collab.generation {
            return;
        }
        let was_live = self.collab.connected;
        self.collab.ws = None;
        self.collab.connected = false;
        self.collab.ready = false;
        self.collab.peers.clear();
        self.collab.last_pub.clear();
        self.sync_ui();
        self.draw();
        if was_live {
            self.toast("Live disconnected — reconnecting");
        } else {
            self.toast("Could not connect — retrying");
        }
        self.schedule_reconnect(generation);
    }

    fn schedule_reconnect(&mut self, generation: u64) {
        if self.collab.room.is_empty() {
            return;
        }
        let delay = self.collab.retry_delay.max(500.0);
        self.collab.retry_delay = (delay * 2.0).min(15_000.0);
        let weak = self.self_ref.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let mut app = app.borrow_mut();
            if app.collab.generation == generation && app.collab.ws.is_none() {
                app.connect_collab();
            }
        });
        let _ = self
            .window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                delay as i32,
            );
        cb.forget();
    }

    fn send_cursor(&mut self, cursor: Option<Pt>) {
        if !self.collab.ready {
            return;
        }
        if self.collab.last_cursor == cursor {
            return;
        }
        let now = js_sys::Date::now();
        if cursor.is_some() && now - self.collab.last_cursor_at < PRESENCE_MS {
            return;
        }
        let message = match cursor {
            Some(point) => serde_json::json!({
                "t": "cursor",
                "x": point.x,
                "y": point.y,
            }),
            None => serde_json::json!({ "t": "cursor", "x": null, "y": null }),
        }
        .to_string();
        if let Some(ws) = &self.collab.ws {
            if ws.send_with_str(&message).is_ok() {
                self.collab.last_cursor = cursor;
                self.collab.last_cursor_at = now;
            }
        }
    }

    fn collab_publish(&mut self, force: bool) {
        if !self.collab.ready {
            return;
        }
        let now = js_sys::Date::now();
        if !force && now - self.collab.last_pub_at < PRESENCE_MS {
            return;
        }
        let current: HashMap<u64, Element> = self
            .elements
            .iter()
            .map(|element| (element.id, element.clone()))
            .collect();
        let mut upsert = Vec::new();
        for (id, element) in &current {
            if self.collab.last_pub.get(id) != Some(element) {
                upsert.push(element.clone());
            }
        }
        let mut delete: Vec<u64> = self
            .collab
            .last_pub
            .keys()
            .filter(|id| !current.contains_key(id))
            .copied()
            .collect();
        if upsert.is_empty() && delete.is_empty() {
            self.collab.last_pub = current;
            return;
        }
        upsert.sort_by_key(|element| element.id);
        delete.sort_unstable();
        let message = serde_json::json!({
            "t": "patch",
            "upsert": upsert,
            "delete": delete,
        })
        .to_string();
        let Some(ws) = &self.collab.ws else {
            return;
        };
        if ws.send_with_str(&message).is_ok() {
            self.collab.last_pub = current;
            self.collab.last_pub_at = now;
        }
    }

    fn save(&mut self) {
        let scene = Scene {
            elements: self.elements.clone(),
            pan: self.pan,
            zoom: self.zoom,
        };
        if let Ok(Some(storage)) = self.window.local_storage() {
            if let Ok(json) = serde_json::to_string(&scene) {
                let _ = storage.set_item(STORAGE_KEY, &json);
            }
        }
        self.collab_publish(true);
    }

    pub fn load(&mut self, shared: &Shared) {
        let mut loaded: Option<Scene> = None;
        if let Ok(location) = self.window.location().hash() {
            if let Some(rest) = location.strip_prefix("#s=") {
                if let Ok(bytes) = URL_SAFE_NO_PAD.decode(rest) {
                    if let Ok(scene) = serde_json::from_slice::<Scene>(&bytes) {
                        loaded = Some(scene);
                    }
                }
            }
        }
        if loaded.is_none() {
            if let Ok(Some(storage)) = self.window.local_storage() {
                if let Ok(Some(json)) = storage.get_item(STORAGE_KEY) {
                    if let Ok(scene) = serde_json::from_str::<Scene>(&json) {
                        loaded = Some(scene);
                    }
                }
            }
        }
        if let Some(scene) = loaded {
            self.elements = scene.elements;
            if scene.zoom >= MIN_ZOOM {
                self.zoom = scene.zoom;
                self.pan = scene.pan;
            }
            self.next_id = self.elements.iter().map(|e| e.id).max().unwrap_or(0);
            for e in self.elements.clone() {
                if e.kind == Kind::Image && !e.src.is_empty() {
                    self.ensure_image(&e.src, shared);
                }
            }
        }
        self.draw();
        self.sync_ui();
    }

    fn ensure_image(&mut self, src: &str, shared: &Shared) {
        if self.images.contains_key(src) {
            return;
        }
        let Ok(node) = self.document.create_element("img") else {
            return;
        };
        let Ok(img) = node.dyn_into::<HtmlImageElement>() else {
            return;
        };
        let app = shared.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            app.borrow_mut().draw();
        });
        img.set_onload(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
        img.set_src(src);
        self.images.insert(src.to_string(), img);
    }

    fn add_image(&mut self, data: String, shared: &Shared) {
        let before = self.elements.clone();
        let pos = self
            .pending_image_pos
            .take()
            .unwrap_or_else(|| self.to_world(Pt::new(self.css_w / 2.0, self.css_h / 2.0)));
        let mut el = self.new_element(Kind::Image, 0.0, 0.0);
        el.src = data.clone();
        el.w = 360.0;
        el.h = 240.0;
        el.x = pos.x - el.w / 2.0;
        el.y = pos.y - el.h / 2.0;
        let id = el.id;
        self.elements.push(el);
        self.selected = vec![id];
        self.history.push(before);
        self.save();
        self.ensure_image(&data, shared);

        let app = shared.clone();
        let data2 = data.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let mut a = app.borrow_mut();
            if let Some(img) = a.images.get(&data2) {
                let nw = img.natural_width() as f64;
                let nh = img.natural_height() as f64;
                if nw > 0.0 && nh > 0.0 {
                    let factor = (520.0 / nw).min(400.0 / nh).min(1.0);
                    if let Some(el) = a.elements.iter_mut().find(|e| e.id == id) {
                        let cx = el.x + el.w / 2.0;
                        let cy = el.y + el.h / 2.0;
                        el.w = nw * factor;
                        el.h = nh * factor;
                        el.x = cx - el.w / 2.0;
                        el.y = cy - el.h / 2.0;
                    }
                }
            }
            a.save();
            a.draw();
        });
        if let Some(img) = self.images.get(&data) {
            img.set_onload(Some(cb.as_ref().unchecked_ref()));
        }
        cb.forget();

        self.tool = Tool::Select;
        self.sync_ui();
        self.draw();
        self.toast("Image added");
    }

    fn toast(&self, message: &str) {
        let Some(el) = self.document.get_element_by_id("toast") else {
            return;
        };
        el.set_text_content(Some(message));
        let _ = el.class_list().add_1("show");
        let doc = self.document.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            if let Some(el) = doc.get_element_by_id("toast") {
                let _ = el.class_list().remove_1("show");
            }
        });
        let _ = self
            .window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                2200,
            );
        cb.forget();
    }

    fn download(&self, filename: &str, mime: &str, content: &[u8]) {
        let encoded = STANDARD.encode(content);
        let href = format!("data:{mime};base64,{encoded}");
        if let Ok(node) = self.document.create_element("a") {
            if let Ok(anchor) = node.dyn_into::<HtmlAnchorElement>() {
                anchor.set_href(&href);
                anchor.set_download(filename);
                anchor.click();
            }
        }
    }

    fn export_json(&self) {
        let scene = Scene {
            elements: self.elements.clone(),
            pan: self.pan,
            zoom: self.zoom,
        };
        if let Ok(json) = serde_json::to_string_pretty(&scene) {
            self.download("forme.json", "application/json", json.as_bytes());
            self.toast("Saved forme.json");
        }
    }

    fn export_svg(&self) {
        if let Some(svg) = render::to_svg(&self.elements) {
            self.download("forme.svg", "image/svg+xml", svg.as_bytes());
            self.toast("SVG exported");
        } else {
            self.toast("Nothing to export");
        }
    }

    fn export_png(&self) {
        if self.elements.is_empty() {
            self.toast("Nothing to export");
            return;
        }
        let mut bounds = self.elements[0].bounds();
        for e in &self.elements[1..] {
            bounds = bounds.union(&e.bounds());
        }
        let pad = 24.0;
        let scale = 2.0;
        let w = ((bounds.w + pad * 2.0) * scale).clamp(1.0, 8000.0);
        let h = ((bounds.h + pad * 2.0) * scale).clamp(1.0, 8000.0);

        let Ok(node) = self.document.create_element("canvas") else {
            return;
        };
        let Ok(canvas) = node.dyn_into::<HtmlCanvasElement>() else {
            return;
        };
        canvas.set_width(w as u32);
        canvas.set_height(h as u32);
        let ctx = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok());
        let Some(ctx) = ctx else {
            return;
        };
        let view = View {
            pan: Pt::new((pad - bounds.x) * scale, (pad - bounds.y) * scale),
            zoom: scale,
            dpr: 1.0,
            w,
            h,
        };
        render::render_scene(&ctx, &self.elements, &self.images, &view, None, None, false);
        if let Ok(url) = canvas.to_data_url() {
            if let Ok(node) = self.document.create_element("a") {
                if let Ok(anchor) = node.dyn_into::<HtmlAnchorElement>() {
                    anchor.set_href(&url);
                    anchor.set_download("forme.png");
                    anchor.click();
                }
            }
            self.toast("PNG exported");
        }
    }

    fn share(&self) {
        let scene = Scene {
            elements: self.elements.clone(),
            pan: self.pan,
            zoom: self.zoom,
        };
        let Ok(json) = serde_json::to_vec(&scene) else {
            return;
        };
        let b64 = URL_SAFE_NO_PAD.encode(json);
        let location = self.window.location();
        let base = format!(
            "{}{}",
            location.origin().unwrap_or_default(),
            location.pathname().unwrap_or_default()
        );
        let url = format!("{base}#s={b64}");
        let _ = self.window.navigator().clipboard().write_text(&url);
        self.toast("Share link copied");
    }

    fn clear_scene(&mut self) {
        if self.elements.is_empty() {
            return;
        }
        let before = self.elements.clone();
        self.elements.clear();
        self.selected.clear();
        self.history.push(before);
        self.save();
        self.sync_ui();
        self.draw();
        self.toast("Canvas cleared");
    }

    fn open_file_picker(&self, id: &str) {
        if let Some(input) = self
            .document
            .get_element_by_id(id)
            .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
        {
            input.click();
        }
    }

    fn load_scene_json(&mut self, raw: &str, shared: &Shared) {
        let parsed: Option<Vec<Element>> = serde_json::from_str::<Scene>(raw)
            .ok()
            .map(|s| s.elements)
            .or_else(|| serde_json::from_str::<Vec<Element>>(raw).ok());
        let Some(elements) = parsed else {
            self.toast("Could not read that file");
            return;
        };
        self.elements = elements;
        self.selected.clear();
        self.next_id = self.elements.iter().map(|e| e.id).max().unwrap_or(0);
        for e in self.elements.clone() {
            if e.kind == Kind::Image && !e.src.is_empty() {
                self.ensure_image(&e.src, shared);
            }
        }
        self.history.clear();
        self.save();
        self.fit_content();
        self.sync_ui();
        self.toast("Drawing loaded");
    }

    pub fn install(app: &Shared) -> Result<(), JsValue> {
        let (window, document, canvas) = {
            let a = app.borrow();
            (a.window.clone(), a.document.clone(), a.canvas.clone())
        };

        for btn in query_all(&document, ".tool[data-tool]") {
            let Some(tool) = btn
                .get_attribute("data-tool")
                .and_then(|v| Tool::from_data(&v))
            else {
                continue;
            };
            let app = app.clone();
            on::<Event>(&btn, "click", move |_| {
                app.borrow_mut().set_tool(tool);
                if tool == Tool::Image {
                    app.borrow().open_file_picker("file-image");
                }
            });
        }

        for (id, action) in [
            ("menu-btn", "toggle-main"),
            ("more-btn", "toggle-more"),
            ("lock", "lock"),
            ("zoom-in", "zoom-in"),
            ("zoom-out", "zoom-out"),
            ("zoom-reset", "zoom-reset"),
            ("undo", "undo"),
            ("redo", "redo"),
            ("share", "share"),
            ("help-btn", "help"),
        ] {
            if let Some(btn) = document.get_element_by_id(id) {
                let app = app.clone();
                on::<Event>(&btn, "click", move |_| {
                    app.borrow_mut().run_action(action);
                });
            }
        }

        for btn in query_all(&document, "[data-action]") {
            let Some(action) = btn.get_attribute("data-action") else {
                continue;
            };
            let app = app.clone();
            on::<Event>(&btn, "click", move |_| {
                app.borrow_mut().run_action(&action);
            });
        }

        for btn in query_all(&document, "[data-stroke]") {
            if let Some(value) = btn.get_attribute("data-stroke") {
                let app = app.clone();
                on::<Event>(&btn, "click", move |_| {
                    app.borrow_mut().set_style_value("stroke", &value);
                });
            }
        }
        for btn in query_all(&document, "[data-bg]") {
            if let Some(value) = btn.get_attribute("data-bg") {
                let app = app.clone();
                on::<Event>(&btn, "click", move |_| {
                    app.borrow_mut().set_style_value("bg", &value);
                });
            }
        }
        for btn in query_all(&document, "[data-fill]") {
            if let Some(value) = btn.get_attribute("data-fill") {
                let app = app.clone();
                on::<Event>(&btn, "click", move |_| {
                    app.borrow_mut().set_style_value("fill", &value);
                });
            }
        }
        for btn in query_all(&document, "[data-width]") {
            if let Some(value) = btn.get_attribute("data-width") {
                let app = app.clone();
                on::<Event>(&btn, "click", move |_| {
                    app.borrow_mut().set_style_value("width", &value);
                });
            }
        }
        for btn in query_all(&document, "[data-font]") {
            if let Some(value) = btn.get_attribute("data-font") {
                let app = app.clone();
                on::<Event>(&btn, "click", move |_| {
                    app.borrow_mut().set_style_value("font", &value);
                });
            }
        }

        {
            let app = app.clone();
            on::<PointerEvent>(&canvas, "pointerdown", move |ev| {
                app.borrow_mut().on_pointer_down(&ev);
            });
        }
        {
            let app = app.clone();
            on::<PointerEvent>(&window, "pointermove", move |ev| {
                app.borrow_mut().on_pointer_move(&ev);
            });
        }
        {
            let app = app.clone();
            on::<PointerEvent>(&window, "pointerup", move |ev| {
                app.borrow_mut().on_pointer_up(&ev);
            });
        }
        {
            let app = app.clone();
            on::<MouseEvent>(&canvas, "contextmenu", move |ev| {
                app.borrow_mut().on_context_menu(&ev);
            });
        }
        {
            let app = app.clone();
            on::<PointerEvent>(&canvas, "pointerleave", move |_| {
                app.borrow_mut().send_cursor(None);
            });
        }
        {
            let app = app.clone();
            on::<WheelEvent>(&canvas, "wheel", move |ev| {
                app.borrow_mut().on_wheel(&ev);
            });
        }
        {
            let app = app.clone();
            on::<KeyboardEvent>(&window, "keydown", move |ev| {
                app.borrow_mut().on_key_down(&ev);
            });
        }
        {
            let app = app.clone();
            on::<KeyboardEvent>(&window, "keyup", move |ev| {
                app.borrow_mut().on_key_up(&ev);
            });
        }
        {
            let app = app.clone();
            on::<Event>(&window, "resize", move |_| {
                app.borrow_mut().draw();
            });
        }

        if let Some(editor) = document.get_element_by_id("text-editor") {
            {
                let app = app.clone();
                on::<KeyboardEvent>(&editor, "keydown", move |ev| {
                    if ev.key() == "Escape" {
                        ev.prevent_default();
                        ev.stop_propagation();
                        app.borrow_mut().commit_text(true);
                    } else if ev.key() == "Enter" && !ev.shift_key() {
                        ev.prevent_default();
                        ev.stop_propagation();
                        app.borrow_mut().commit_text(false);
                    } else {
                        ev.stop_propagation();
                    }
                });
            }
            {
                let app = app.clone();
                on::<Event>(&editor, "blur", move |_| {
                    app.borrow_mut().commit_text(false);
                });
            }
        }

        for (id, action) in [("file-open", "open-loaded"), ("file-image", "image-loaded")] {
            if let Some(input) = document
                .get_element_by_id(id)
                .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
            {
                let app = app.clone();
                let action = action.to_string();
                let input2 = input.clone();
                on::<Event>(&input, "change", move |_| {
                    read_input_file(&input2, &action, &app);
                });
            }
        }

        if let Some(dialog) = document.get_element_by_id("help-dialog") {
            if let Some(close) = document.get_element_by_id("help-close") {
                let dialog = dialog.clone();
                on::<Event>(&close, "click", move |_| {
                    if let Ok(dialog) = dialog.clone().dyn_into::<HtmlDialogElement>() {
                        dialog.close();
                    }
                });
            }
        }

        Ok(())
    }

    fn run_action(&mut self, action: &str) {
        match action {
            "toggle-main" => toggle_menu(&self.document, "main-menu"),
            "toggle-more" => toggle_menu(&self.document, "more-menu"),
            "lock" => {
                self.locked = !self.locked;
                self.sync_ui();
            }
            "zoom-in" => self.zoom_by(1.15),
            "zoom-out" => self.zoom_by(1.0 / 1.15),
            "zoom-reset" => self.fit_content(),
            "undo" => self.undo(),
            "redo" => self.redo(),
            "share" => self.share(),
            "collab" => {
                self.close_menus();
                self.start_collab();
            }
            "new" | "clear" => {
                self.close_menus();
                self.clear_scene();
            }
            "open" => {
                self.close_menus();
                self.open_file_picker("file-open");
            }
            "save" => {
                self.close_menus();
                self.export_json();
            }
            "png" => {
                self.close_menus();
                self.export_png();
            }
            "svg" => {
                self.close_menus();
                self.export_svg();
            }
            "grid" => {
                self.show_grid = !self.show_grid;
                self.close_menus();
                self.draw();
            }
            "duplicate" => {
                self.close_menus();
                self.duplicate();
            }
            "delete" => {
                self.close_menus();
                self.delete_selected();
            }
            "copy" => {
                self.close_menus();
                self.copy();
            }
            "cut" => {
                self.close_menus();
                self.copy();
                self.delete_selected();
            }
            "paste" => {
                self.close_menus();
                self.paste();
            }
            "select-all" => {
                self.close_menus();
                self.selected = self.elements.iter().map(|e| e.id).collect();
                self.sync_ui();
                self.draw();
            }
            "help" => {
                if let Some(dialog) = self.document.get_element_by_id("help-dialog") {
                    if let Ok(dialog) = dialog.dyn_into::<HtmlDialogElement>() {
                        let _ = dialog.show_modal();
                    }
                }
            }
            _ => {}
        }
    }
}

fn resize_rect(bounds: Rect, p: Pt, handle: usize) -> Rect {
    let mut left = bounds.x;
    let mut top = bounds.y;
    let mut right = bounds.right();
    let mut bottom = bounds.bottom();
    match handle {
        0 => {
            left = p.x;
            top = p.y;
        }
        1 => top = p.y,
        2 => {
            right = p.x;
            top = p.y;
        }
        3 => right = p.x,
        4 => {
            right = p.x;
            bottom = p.y;
        }
        5 => bottom = p.y,
        6 => {
            left = p.x;
            bottom = p.y;
        }
        7 => left = p.x,
        _ => {}
    }
    Rect::from_xywh(
        left.min(right),
        top.min(bottom),
        (right - left).abs().max(1.0),
        (bottom - top).abs().max(1.0),
    )
}

fn snap_angle(start: Pt, end: Pt) -> Pt {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let len = (dx * dx + dy * dy).sqrt();
    let step = std::f64::consts::PI / 4.0;
    let angle = (dy.atan2(dx) / step).round() * step;
    Pt::new(start.x + len * angle.cos(), start.y + len * angle.sin())
}

fn query_all(document: &Document, selector: &str) -> Vec<HtmlElement> {
    let mut out = Vec::new();
    if let Ok(list) = document.query_selector_all(selector) {
        collect_nodes(&list, &mut out);
    }
    out
}

fn collect_nodes(list: &NodeList, out: &mut Vec<HtmlElement>) {
    for i in 0..list.length() {
        if let Some(node) = list.get(i) {
            if let Ok(el) = node.dyn_into::<HtmlElement>() {
                out.push(el);
            }
        }
    }
}

fn set_active(el: &DomElement, active: bool) {
    if active {
        let _ = el.class_list().add_1("active");
    } else {
        let _ = el.class_list().remove_1("active");
    }
}

fn toggle_disabled(el: &DomElement, disabled: bool) {
    if disabled {
        let _ = el.set_attribute("disabled", "");
    } else {
        let _ = el.remove_attribute("disabled");
    }
}

fn toggle_menu(document: &Document, id: &str) {
    for other in ["main-menu", "more-menu", "context-menu"] {
        if other != id {
            if let Some(el) = document.get_element_by_id(other) {
                let _ = el.class_list().remove_1("open");
            }
        }
    }
    if let Some(el) = document.get_element_by_id(id) {
        let _ = el.class_list().toggle("open");
    }
}

fn on<E: JsCast + FromWasmAbi + 'static>(
    target: &EventTarget,
    event: &str,
    f: impl FnMut(E) + 'static,
) {
    let cb: Closure<dyn FnMut(E)> = Closure::new(f);
    let _ = target.add_event_listener_with_callback(event, cb.as_ref().unchecked_ref());
    cb.forget();
}

fn read_input_file(input: &HtmlInputElement, action: &str, app: &Shared) {
    let Some(files) = input.files() else {
        return;
    };
    let Some(file) = files.get(0) else {
        return;
    };
    let Ok(reader) = FileReader::new() else {
        return;
    };
    let r2 = reader.clone();
    let action = action.to_string();
    let is_open = action == "open-loaded";
    let app = app.clone();
    let cb = Closure::<dyn FnMut(Event)>::new(move |_| {
        if let Ok(value) = r2.result() {
            if let Some(text) = value.as_string() {
                let mut a = app.borrow_mut();
                if is_open {
                    a.load_scene_json(&text, &app);
                } else {
                    a.add_image(text, &app);
                }
            }
        }
    });
    reader.set_onload(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
    if is_open {
        let _ = reader.read_as_text(&file);
    } else {
        let _ = reader.read_as_data_url(&file);
    }
    input.set_value("");
}
