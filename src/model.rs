use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn from_points(a: Pt, b: Pt) -> Self {
        Self {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            w: (a.x - b.x).abs(),
            h: (a.y - b.y).abs(),
        }
    }

    pub fn from_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn center(&self) -> Pt {
        Pt::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn inflate(&self, n: f64) -> Self {
        Self {
            x: self.x - n,
            y: self.y - n,
            w: self.w + n * 2.0,
            h: self.h + n * 2.0,
        }
    }

    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        !(other.x > self.right()
            || other.right() < self.x
            || other.y > self.bottom()
            || other.bottom() < self.y)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = self.right().max(other.right());
        let b = self.bottom().max(other.bottom());
        Rect::from_xywh(x, y, r - x, b - y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Rectangle,
    Diamond,
    Ellipse,
    Arrow,
    Line,
    Draw,
    Text,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fill {
    Hachure,
    Cross,
    Solid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub stroke: String,
    pub bg: String,
    pub fill: Fill,
    pub width: f64,
    pub font: f64,
    pub opacity: f64,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            stroke: "#1e1e1e".to_string(),
            bg: "transparent".to_string(),
            fill: Fill::Hachure,
            width: 2.0,
            font: 20.0,
            opacity: 100.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: u64,
    pub kind: Kind,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub points: Vec<Pt>,
    pub style: Style,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub src: String,
    pub seed: u32,
}

impl Element {
    pub fn new(id: u64, kind: Kind, x: f64, y: f64, style: Style, seed: u32) -> Self {
        Self {
            id,
            kind,
            x,
            y,
            w: 0.0,
            h: 0.0,
            points: Vec::new(),
            style,
            text: String::new(),
            src: String::new(),
            seed,
        }
    }

    pub fn bounds(&self) -> Rect {
        if matches!(self.kind, Kind::Line | Kind::Arrow | Kind::Draw) && !self.points.is_empty() {
            let mut min_x = f64::MAX;
            let mut min_y = f64::MAX;
            let mut max_x = f64::MIN;
            let mut max_y = f64::MIN;
            for p in &self.points {
                min_x = min_x.min(p.x);
                min_y = min_y.min(p.y);
                max_x = max_x.max(p.x);
                max_y = max_y.max(p.y);
            }
            Rect::from_xywh(min_x, min_y, max_x - min_x, max_y - min_y)
        } else {
            Rect::from_xywh(self.x, self.y, self.w, self.h)
        }
    }

    pub fn sync_bounds(&mut self) {
        let bounds = self.bounds();
        self.x = bounds.x;
        self.y = bounds.y;
        self.w = bounds.w;
        self.h = bounds.h;
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        for p in &mut self.points {
            p.x += dx;
            p.y += dy;
        }
    }

    pub fn is_linear(&self) -> bool {
        matches!(self.kind, Kind::Line | Kind::Arrow | Kind::Draw)
    }

    pub fn set_from_rect(&mut self, r: Rect) {
        self.x = r.x;
        self.y = r.y;
        self.w = r.w;
        self.h = r.h;
    }
}

#[derive(Default)]
pub struct History {
    undo: Vec<Vec<Element>>,
    redo: Vec<Vec<Element>>,
}

impl History {
    pub fn push(&mut self, snapshot: Vec<Element>) {
        if self.undo.last() == Some(&snapshot) {
            self.redo.clear();
            return;
        }
        self.undo.push(snapshot);
        if self.undo.len() > 80 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: Vec<Element>) -> Option<Vec<Element>> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        Some(prev)
    }

    pub fn redo(&mut self, current: Vec<Element>) -> Option<Vec<Element>> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Hand,
    Rectangle,
    Diamond,
    Ellipse,
    Arrow,
    Line,
    Draw,
    Text,
    Image,
    Eraser,
}

impl Tool {
    pub fn from_data(value: &str) -> Option<Self> {
        Some(match value {
            "select" => Tool::Select,
            "hand" => Tool::Hand,
            "rectangle" => Tool::Rectangle,
            "diamond" => Tool::Diamond,
            "ellipse" => Tool::Ellipse,
            "arrow" => Tool::Arrow,
            "line" => Tool::Line,
            "draw" => Tool::Draw,
            "text" => Tool::Text,
            "image" => Tool::Image,
            "eraser" => Tool::Eraser,
            _ => return None,
        })
    }
}

pub fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq <= f64::EPSILON {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    let cx = a.x + t * dx;
    let cy = a.y + t * dy;
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_bounds_tracks_freehand_points() {
        let mut element = Element::new(1, Kind::Draw, 0.0, 0.0, Style::default(), 7);
        element.points = vec![Pt::new(10.0, 20.0), Pt::new(15.0, 32.0)];
        element.sync_bounds();
        assert_eq!(element.x, 10.0);
        assert_eq!(element.y, 20.0);
        assert_eq!(element.w, 5.0);
        assert_eq!(element.h, 12.0);
    }
}
