use crate::model::{Pt, Rect};

pub fn simplify_stroke(points: &[Pt], tolerance: f64) -> Vec<Pt> {
    if points.is_empty() {
        return Vec::new();
    }

    let mut clean = Vec::with_capacity(points.len());
    for &point in points {
        if !point.x.is_finite() || !point.y.is_finite() {
            continue;
        }
        let duplicate = clean
            .last()
            .is_some_and(|last| distance_sq(*last, point) <= 1e-12);
        if !duplicate {
            clean.push(point);
        }
    }
    if clean.len() <= 2 {
        return clean;
    }

    let tolerance = tolerance.max(0.0);
    let limit = tolerance * tolerance;
    let last = clean.len() - 1;
    let mut keep = vec![false; clean.len()];
    keep[0] = true;
    keep[last] = true;
    let mut pending = vec![(0usize, last)];

    while let Some((start, end)) = pending.pop() {
        if end <= start + 1 {
            continue;
        }
        let mut farthest = 0.0;
        let mut split = None;
        for index in start + 1..end {
            let distance = segment_distance_sq(clean[index], clean[start], clean[end]);
            if distance > farthest {
                farthest = distance;
                split = Some(index);
            }
        }
        if farthest > limit {
            if let Some(index) = split {
                keep[index] = true;
                pending.push((start, index));
                pending.push((index, end));
            }
        }
    }

    clean
        .into_iter()
        .enumerate()
        .filter_map(|(index, point)| keep[index].then_some(point))
        .collect()
}

pub fn viewport_rect(pan: Pt, zoom: f64, width: f64, height: f64) -> Rect {
    let zoom = zoom.max(1e-9);
    Rect::from_xywh(
        -pan.x / zoom,
        -pan.y / zoom,
        width.max(0.0) / zoom,
        height.max(0.0) / zoom,
    )
}

pub fn is_visible(bounds: Rect, view: Rect, margin: f64) -> bool {
    bounds.inflate(margin.max(0.0)).intersects(&view)
}

fn distance_sq(a: Pt, b: Pt) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}

fn segment_distance_sq(point: Pt, start: Pt, end: Pt) -> f64 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_sq = dx * dx + dy * dy;
    if length_sq <= f64::EPSILON {
        return distance_sq(point, start);
    }
    let t = (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_sq).clamp(0.0, 1.0);
    let nearest = Pt::new(start.x + t * dx, start.y + t * dy);
    distance_sq(point, nearest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(values: &[(f64, f64)]) -> Vec<Pt> {
        values.iter().map(|&(x, y)| Pt::new(x, y)).collect()
    }

    #[test]
    fn viewport_rect_translates_and_scales() {
        let view = viewport_rect(Pt::new(-100.0, -50.0), 2.0, 800.0, 600.0);
        assert_eq!(view, Rect::from_xywh(50.0, 25.0, 400.0, 300.0));
    }

    #[test]
    fn visibility_includes_margin() {
        let view = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        assert!(is_visible(
            Rect::from_xywh(90.0, 90.0, 20.0, 20.0),
            view,
            0.0
        ));
        assert!(!is_visible(
            Rect::from_xywh(110.0, 10.0, 20.0, 20.0),
            view,
            0.0
        ));
        assert!(is_visible(
            Rect::from_xywh(110.0, 10.0, 20.0, 20.0),
            view,
            10.0
        ));
    }

    #[test]
    fn simplify_drops_collinear_points() {
        let input = pts(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0)]);
        assert_eq!(simplify_stroke(&input, 0.1), pts(&[(0.0, 0.0), (3.0, 0.0)]));
    }

    #[test]
    fn simplify_keeps_corners() {
        let input = pts(&[(0.0, 0.0), (5.0, 0.0), (5.0, 5.0)]);
        assert_eq!(simplify_stroke(&input, 0.01), input);
    }

    #[test]
    fn simplify_honors_tolerance() {
        let input = pts(&[(0.0, 0.0), (5.0, 0.5), (10.0, 0.0)]);
        assert_eq!(simplify_stroke(&input, 0.25), input);
        assert_eq!(
            simplify_stroke(&input, 1.0),
            pts(&[(0.0, 0.0), (10.0, 0.0)])
        );
    }

    #[test]
    fn simplify_keeps_short_strokes() {
        assert_eq!(simplify_stroke(&[], 1.0), Vec::<Pt>::new());
        let one = pts(&[(2.0, 3.0)]);
        assert_eq!(simplify_stroke(&one, 1.0), one);
        let two = pts(&[(0.0, 0.0), (1.0, 1.0)]);
        assert_eq!(simplify_stroke(&two, 1.0), two);
    }

    #[test]
    fn simplify_removes_duplicate_points() {
        let input = pts(&[(0.0, 0.0), (0.0, 0.0), (10.0, 0.0), (10.0, 0.0)]);
        assert_eq!(
            simplify_stroke(&input, 0.0),
            pts(&[(0.0, 0.0), (10.0, 0.0)])
        );
    }

    #[test]
    fn simplify_shrinks_noisy_stroke() {
        let input = pts(&[
            (0.0, 0.0),
            (1.0, 0.02),
            (2.0, -0.03),
            (3.0, 0.01),
            (4.0, 0.0),
            (5.0, 0.02),
            (6.0, 0.0),
        ]);
        let output = simplify_stroke(&input, 0.2);
        assert_eq!(output.len(), 2);
        assert_eq!(output.first(), input.first());
        assert_eq!(output.last(), input.last());
    }
}
