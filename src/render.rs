use std::collections::HashMap;

use js_sys::Array;
use wasm_bindgen::JsValue;
use web_sys::{CanvasRenderingContext2d, HtmlImageElement};

use crate::geometry::{is_visible, viewport_rect};
use crate::model::{Element, Fill, Kind, Pt, Rect};

pub const FONT: &str = "Caveat, 'Comic Sans MS', 'Segoe Print', cursive";
pub const SELECT: &str = "#6965db";

pub struct View {
    pub pan: Pt,
    pub zoom: f64,
    pub dpr: f64,
    pub w: f64,
    pub h: f64,
}

#[allow(clippy::too_many_arguments)]
pub fn render_scene(
    ctx: &CanvasRenderingContext2d,
    elements: &[Element],
    images: &HashMap<String, HtmlImageElement>,
    view: &View,
    selection: Option<Rect>,
    marquee: Option<Rect>,
    grid: bool,
) {
    let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    ctx.set_fill_style_str("#ffffff");
    ctx.fill_rect(0.0, 0.0, view.w, view.h);

    let _ = ctx.set_transform(
        view.dpr * view.zoom,
        0.0,
        0.0,
        view.dpr * view.zoom,
        view.dpr * view.pan.x,
        view.dpr * view.pan.y,
    );

    if grid {
        draw_grid(ctx, view);
    }

    let view_bounds = viewport_rect(view.pan, view.zoom, view.w, view.h);
    let cull_margin = 12.0 / view.zoom.max(0.01);
    for element in elements {
        if !is_visible(element.bounds(), view_bounds, cull_margin) {
            continue;
        }
        draw_element(ctx, element, images);
    }

    if let Some(bounds) = selection {
        draw_selection(ctx, bounds, view.zoom);
    }

    if let Some(rect) = marquee {
        ctx.set_fill_style_str("rgba(105, 101, 219, 0.10)");
        ctx.set_stroke_style_str(SELECT);
        ctx.set_line_width(1.0 / view.zoom);
        ctx.fill_rect(rect.x, rect.y, rect.w, rect.h);
        ctx.stroke_rect(rect.x, rect.y, rect.w, rect.h);
    }
}

pub fn draw_grid(ctx: &CanvasRenderingContext2d, view: &View) {
    if view.zoom < 0.35 {
        return;
    }
    let spacing = 24.0;
    let left = (-view.pan.x / view.zoom) / spacing;
    let top = (-view.pan.y / view.zoom) / spacing;
    let right = ((view.w - view.pan.x) / view.zoom) / spacing;
    let bottom = ((view.h - view.pan.y) / view.zoom) / spacing;

    let start_x = left.floor() as i64;
    let start_y = top.floor() as i64;
    let end_x = right.ceil() as i64;
    let end_y = bottom.ceil() as i64;
    if (end_x - start_x) * (end_y - start_y) > 12_000 {
        return;
    }

    let size = (1.3 / view.zoom).max(0.4);
    ctx.set_fill_style_str("#dee2e6");
    for gy in start_y..=end_y {
        for gx in start_x..=end_x {
            ctx.fill_rect(
                gx as f64 * spacing - size / 2.0,
                gy as f64 * spacing - size / 2.0,
                size,
                size,
            );
        }
    }
}

fn lcg(seed: &mut u32) -> f64 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    ((*seed >> 8) & 0x00FF_FFFF) as f64 / 16_777_216.0
}

fn jitter(p: Pt, seed: &mut u32, amp: f64) -> Pt {
    Pt::new(
        p.x + (lcg(seed) * 2.0 - 1.0) * amp,
        p.y + (lcg(seed) * 2.0 - 1.0) * amp,
    )
}

fn rough_line(
    ctx: &CanvasRenderingContext2d,
    pts: &[Pt],
    closed: bool,
    seed: u32,
    amp: f64,
    passes: u32,
) {
    for pass in 0..passes {
        let mut s = seed.wrapping_add(pass.wrapping_mul(0x9E37_79B9));
        ctx.begin_path();
        let first = jitter(pts[0], &mut s, amp);
        ctx.move_to(first.x, first.y);
        let mut prev = first;
        for p in &pts[1..] {
            let end = jitter(*p, &mut s, amp);
            let mid = Pt::new((prev.x + end.x) / 2.0, (prev.y + end.y) / 2.0);
            let ctrl = jitter(mid, &mut s, amp * 1.35);
            ctx.quadratic_curve_to(ctrl.x, ctrl.y, end.x, end.y);
            prev = end;
        }
        if closed {
            ctx.close_path();
        }
        ctx.stroke();
    }
}

fn ellipse_points(r: Rect) -> Vec<Pt> {
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    let rx = (r.w / 2.0).max(0.1);
    let ry = (r.h / 2.0).max(0.1);
    let n = 44;
    (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            Pt::new(cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect()
}

fn diamond_points(r: Rect) -> Vec<Pt> {
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    vec![
        Pt::new(cx, r.y),
        Pt::new(r.right(), cy),
        Pt::new(cx, r.bottom()),
        Pt::new(r.x, cy),
    ]
}

fn shape_path(ctx: &CanvasRenderingContext2d, e: &Element) {
    let r = e.bounds();
    match e.kind {
        Kind::Rectangle | Kind::Image => {
            ctx.rect(r.x, r.y, r.w.max(0.1), r.h.max(0.1));
        }
        Kind::Diamond => {
            let pts = diamond_points(r);
            ctx.move_to(pts[0].x, pts[0].y);
            for p in &pts[1..] {
                ctx.line_to(p.x, p.y);
            }
            ctx.close_path();
        }
        Kind::Ellipse => {
            let _ = ctx.ellipse(
                r.x + r.w / 2.0,
                r.y + r.h / 2.0,
                (r.w / 2.0).max(0.1),
                (r.h / 2.0).max(0.1),
                0.0,
                0.0,
                std::f64::consts::TAU,
            );
        }
        _ => {}
    }
}

fn paint_fill(ctx: &CanvasRenderingContext2d, e: &Element) {
    if e.style.bg.is_empty() || e.style.bg == "transparent" {
        return;
    }
    if matches!(e.kind, Kind::Line | Kind::Arrow | Kind::Draw | Kind::Text) {
        return;
    }

    let r = e.bounds().inflate(-0.5);

    if e.style.fill == Fill::Solid {
        ctx.begin_path();
        shape_path(ctx, e);
        ctx.set_fill_style_str(&e.style.bg);
        ctx.fill();
        return;
    }

    ctx.save();
    ctx.begin_path();
    shape_path(ctx, e);
    ctx.clip();
    ctx.set_stroke_style_str(&e.style.bg);
    ctx.set_line_width(1.1);
    let spacing = 7.0;
    let diagonal = r.w.max(r.h) + 8.0;
    let mut t = -diagonal;
    while t < diagonal {
        let mut s = e.seed.wrapping_add((t * 13.0) as u32);
        let x0 = r.x + t + (lcg(&mut s) - 0.5) * 2.0;
        ctx.begin_path();
        ctx.move_to(x0, r.y - 8.0);
        ctx.line_to(x0 + r.h + 16.0, r.bottom() + 8.0);
        ctx.stroke();
        t += spacing;
    }
    if e.style.fill == Fill::Cross {
        let mut t = -diagonal;
        while t < diagonal {
            let mut s = e.seed.wrapping_add((t * 31.0) as u32);
            let x0 = r.x + t + (lcg(&mut s) - 0.5) * 2.0;
            ctx.begin_path();
            ctx.move_to(x0, r.bottom() + 8.0);
            ctx.line_to(x0 + r.h + 16.0, r.y - 8.0);
            ctx.stroke();
            t += spacing;
        }
    }
    ctx.restore();
}

fn draw_element(
    ctx: &CanvasRenderingContext2d,
    e: &Element,
    images: &HashMap<String, HtmlImageElement>,
) {
    if e.kind == Kind::Image {
        if let Some(img) = images.get(&e.src) {
            if img.complete() && img.natural_width() > 0 {
                let r = e.bounds();
                let _ =
                    ctx.draw_image_with_html_image_element_and_dw_and_dh(img, r.x, r.y, r.w, r.h);
            }
        }
        return;
    }

    if e.kind == Kind::Text {
        draw_text(ctx, e);
        return;
    }

    ctx.save();
    ctx.set_global_alpha((e.style.opacity / 100.0).clamp(0.05, 1.0));
    ctx.set_line_cap("round");
    ctx.set_line_join("round");

    paint_fill(ctx, e);

    ctx.set_stroke_style_str(&e.style.stroke);
    ctx.set_line_width(e.style.width.max(0.5));

    let r = e.bounds();
    let amp = (1.15 + e.style.width * 0.15).min(2.4);

    match e.kind {
        Kind::Rectangle => {
            let pts = vec![
                Pt::new(r.x, r.y),
                Pt::new(r.right(), r.y),
                Pt::new(r.right(), r.bottom()),
                Pt::new(r.x, r.bottom()),
            ];
            rough_line(ctx, &pts, true, e.seed, amp, 2);
        }
        Kind::Diamond => {
            let pts = diamond_points(r);
            rough_line(ctx, &pts, true, e.seed, amp, 2);
        }
        Kind::Ellipse => {
            let pts = ellipse_points(r);
            rough_line(ctx, &pts, true, e.seed, amp, 2);
        }
        Kind::Line | Kind::Arrow => {
            if e.points.len() >= 2 {
                rough_line(ctx, &e.points, false, e.seed, amp, 2);
                if e.kind == Kind::Arrow {
                    draw_arrow_head(ctx, &e.points, e.style.width);
                }
            }
        }
        Kind::Draw => draw_freehand(ctx, &e.points),
        _ => {}
    }

    ctx.restore();
}

fn draw_arrow_head(ctx: &CanvasRenderingContext2d, points: &[Pt], width: f64) {
    let n = points.len();
    if n < 2 {
        return;
    }
    let tip = points[n - 1];
    let prev = points[n - 2];
    let angle = (tip.y - prev.y).atan2(tip.x - prev.x);
    let len = 11.0 + width * 2.2;
    let spread = 0.48;

    ctx.begin_path();
    ctx.move_to(
        tip.x - len * (angle - spread).cos(),
        tip.y - len * (angle - spread).sin(),
    );
    ctx.line_to(tip.x, tip.y);
    ctx.line_to(
        tip.x - len * (angle + spread).cos(),
        tip.y - len * (angle + spread).sin(),
    );
    ctx.stroke();
}

fn draw_freehand(ctx: &CanvasRenderingContext2d, points: &[Pt]) {
    if points.is_empty() {
        return;
    }
    ctx.begin_path();
    if points.len() == 1 {
        ctx.arc(points[0].x, points[0].y, 1.0, 0.0, std::f64::consts::TAU)
            .ok();
        ctx.fill();
        return;
    }
    ctx.move_to(points[0].x, points[0].y);
    if points.len() == 2 {
        ctx.line_to(points[1].x, points[1].y);
    } else {
        for i in 1..points.len() - 1 {
            let mid = Pt::new(
                (points[i].x + points[i + 1].x) / 2.0,
                (points[i].y + points[i + 1].y) / 2.0,
            );
            ctx.quadratic_curve_to(points[i].x, points[i].y, mid.x, mid.y);
        }
        let last = points[points.len() - 1];
        ctx.line_to(last.x, last.y);
    }
    ctx.stroke();
}

fn draw_text(ctx: &CanvasRenderingContext2d, e: &Element) {
    if e.text.is_empty() {
        return;
    }
    ctx.save();
    ctx.set_global_alpha((e.style.opacity / 100.0).clamp(0.05, 1.0));
    ctx.set_font(&format!("{}px {}", e.style.font, FONT));
    ctx.set_fill_style_str(&e.style.stroke);
    ctx.set_text_baseline("top");
    for (i, line) in e.text.split('\n').enumerate() {
        let _ = ctx.fill_text(line, e.x, e.y + i as f64 * e.style.font * 1.25);
    }
    ctx.restore();
}

pub fn handle_rects(bounds: Rect, zoom: f64) -> Vec<Rect> {
    let s = 9.0 / zoom;
    let half = s / 2.0;
    let cx = bounds.x + bounds.w / 2.0;
    let cy = bounds.y + bounds.h / 2.0;
    let positions = [
        (bounds.x, bounds.y),
        (cx, bounds.y),
        (bounds.right(), bounds.y),
        (bounds.right(), cy),
        (bounds.right(), bounds.bottom()),
        (cx, bounds.bottom()),
        (bounds.x, bounds.bottom()),
        (bounds.x, cy),
    ];
    positions
        .iter()
        .map(|(x, y)| Rect::from_xywh(x - half, y - half, s, s))
        .collect()
}

fn draw_selection(ctx: &CanvasRenderingContext2d, bounds: Rect, zoom: f64) {
    ctx.save();
    ctx.set_stroke_style_str(SELECT);
    ctx.set_line_width(1.0 / zoom);
    let dash = Array::new();
    dash.push(&JsValue::from(5.0 / zoom));
    dash.push(&JsValue::from(4.0 / zoom));
    let _ = ctx.set_line_dash(&dash);
    let b = bounds.inflate(1.0 / zoom);
    ctx.stroke_rect(b.x, b.y, b.w, b.h);
    let _ = ctx.set_line_dash(&Array::new());

    ctx.set_fill_style_str("#ffffff");
    ctx.set_line_width(1.3 / zoom);
    for h in handle_rects(bounds, zoom) {
        ctx.fill_rect(h.x, h.y, h.w, h.h);
        ctx.stroke_rect(h.x, h.y, h.w, h.h);
    }
    ctx.restore();
}

pub fn escaping(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn to_svg(elements: &[Element]) -> Option<String> {
    if elements.is_empty() {
        return None;
    }
    let mut bounds = elements[0].bounds();
    for e in &elements[1..] {
        bounds = bounds.union(&e.bounds());
    }
    let pad = 24.0;
    let x = bounds.x - pad;
    let y = bounds.y - pad;
    let w = bounds.w + pad * 2.0;
    let h = bounds.h + pad * 2.0;

    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"{x:.2} {y:.2} {w:.2} {h:.2}\"><rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{h:.2}\" fill=\"#ffffff\"/>"
    );

    for e in elements {
        let r = e.bounds();
        let fill = if e.style.bg.is_empty() || e.style.bg == "transparent" {
            "none".to_string()
        } else {
            e.style.bg.clone()
        };
        let stroke = &e.style.stroke;
        let sw = e.style.width;
        let opacity = e.style.opacity / 100.0;
        match e.kind {
            Kind::Rectangle => out.push_str(&format!(
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" rx=\"3\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\" opacity=\"{}\"/>",
                r.x, r.y, r.w, r.h, fill, stroke, sw, opacity
            )),
            Kind::Image => {
                if !e.src.is_empty() {
                    out.push_str(&format!(
                        "<image href=\"{}\" x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" preserveAspectRatio=\"none\" opacity=\"{}\"/>",
                        escaping(&e.src), r.x, r.y, r.w, r.h, opacity
                    ));
                } else {
                    out.push_str(&format!(
                        "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" rx=\"3\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" opacity=\"{}\"/>",
                        r.x, r.y, r.w, r.h, stroke, sw, opacity
                    ));
                }
            }
            Kind::Diamond => {
                let p = format!(
                    "{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
                    r.x + r.w / 2.0,
                    r.y,
                    r.right(),
                    r.y + r.h / 2.0,
                    r.x + r.w / 2.0,
                    r.bottom(),
                    r.x,
                    r.y + r.h / 2.0
                );
                out.push_str(&format!(
                    "<polygon points=\"{p}\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{sw}\" opacity=\"{opacity}\"/>"
                ));
            }
            Kind::Ellipse => out.push_str(&format!(
                "<ellipse cx=\"{:.2}\" cy=\"{:.2}\" rx=\"{:.2}\" ry=\"{:.2}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\" opacity=\"{}\"/>",
                r.x + r.w / 2.0,
                r.y + r.h / 2.0,
                r.w / 2.0,
                r.h / 2.0,
                fill,
                stroke,
                sw,
                opacity
            )),
            Kind::Line | Kind::Arrow => {
                if e.points.len() >= 2 {
                    let a = e.points[0];
                    let b = e.points[e.points.len() - 1];
                    out.push_str(&format!(
                        "<line x1=\"{:.2}\" y1=\"{:.2}\" x2=\"{:.2}\" y2=\"{:.2}\" stroke=\"{stroke}\" stroke-width=\"{sw}\" stroke-linecap=\"round\" opacity=\"{opacity}\"/>",
                        a.x, a.y, b.x, b.y
                    ));
                    if e.kind == Kind::Arrow {
                        let angle = (b.y - a.y).atan2(b.x - a.x);
                        let len = 11.0 + sw * 2.2;
                        let spread = 0.48;
                        let p1 = (
                            b.x - len * (angle - spread).cos(),
                            b.y - len * (angle - spread).sin(),
                        );
                        let p2 = (
                            b.x - len * (angle + spread).cos(),
                            b.y - len * (angle + spread).sin(),
                        );
                        out.push_str(&format!(
                            "<polyline points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{sw}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" opacity=\"{opacity}\"/>",
                            p1.0, p1.1, b.x, b.y, p2.0, p2.1
                        ));
                    }
                }
            }
            Kind::Draw => {
                if e.points.len() >= 2 {
                    let points: Vec<String> = e
                        .points
                        .iter()
                        .map(|p| format!("{:.2},{:.2}", p.x, p.y))
                        .collect();
                    out.push_str(&format!(
                        "<polyline points=\"{}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"{sw}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" opacity=\"{opacity}\"/>",
                        points.join(" ")
                    ));
                }
            }
            Kind::Text => {
                for (i, line) in e.text.split('\n').enumerate() {
                    out.push_str(&format!(
                        "<text x=\"{:.2}\" y=\"{:.2}\" font-family=\"Caveat, Comic Sans MS, cursive\" font-size=\"{}\" fill=\"{stroke}\" opacity=\"{opacity}\">{}</text>",
                        e.x,
                        e.y + e.style.font + i as f64 * e.style.font * 1.25,
                        e.style.font,
                        escaping(line)
                    ));
                }
            }
        }
    }

    out.push_str("</svg>");
    Some(out)
}
