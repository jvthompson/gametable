//! Shapes turned into triangles exactly the way LÖVE 11 does it (`Graphics.cpp`, `Polyline.cpp`),
//! so rounded corners, circles, arcs and lines have the same number of segments and the same
//! joins as in the original.
//!
//! Points are in the caller's coordinate space (before the transform). Everything is `f32` like
//! LÖVE's own maths.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// A 2D point or vector.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// LÖVE's `Vector2::getNormal(scale)`: the vector turned a quarter and scaled.
    fn normal(self, scale: f32) -> Point {
        Point::new(-self.y * scale, self.x * scale)
    }

    fn normalized_to(self, length: f32) -> Point {
        let current = self.length();
        if current > 0.0 { self * (length / current) } else { self }
    }

    fn cross(self, other: Point) -> f32 {
        self.x * other.y - self.y * other.x
    }

    fn dot(self, other: Point) -> f32 {
        self.x * other.x + self.y * other.y
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, other: Point) -> Point {
        Point::new(self.x + other.x, self.y + other.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Point;
    fn sub(self, other: Point) -> Point {
        Point::new(self.x - other.x, self.y - other.y)
    }
}

impl std::ops::Mul<f32> for Point {
    type Output = Point;
    fn mul(self, scale: f32) -> Point {
        Point::new(self.x * scale, self.y * scale)
    }
}

impl std::ops::Neg for Point {
    type Output = Point;
    fn neg(self) -> Point {
        Point::new(-self.x, -self.y)
    }
}

/// `"fill"` or `"line"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawMode {
    Fill,
    Line,
}

/// How an arc's ends are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArcMode {
    /// Lines to the centre (LÖVE's default).
    #[default]
    Pie,
    /// Ends left unconnected.
    Open,
    /// Ends joined by a straight line.
    Closed,
}

/// How outlines are drawn: half the line width, and whether LÖVE's "smooth" feather is added.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineStyle {
    pub half_width: f32,
    pub smooth: bool,
}

impl LineStyle {
    pub fn new(width: f32, smooth: bool) -> LineStyle {
        LineStyle { half_width: width * 0.5, smooth }
    }
}

/// A vertex with an alpha multiplier (line overdraw fades its outer edge to transparent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeVertex {
    pub position: Point,
    pub alpha: f32,
}

/// Triangles ready to draw: every three vertices form one triangle.
pub type Triangles = Vec<ShapeVertex>;

/// LÖVE's `calculateEllipsePoints` with a pixel scale of 1 (`highdpi` is off in the original).
pub fn ellipse_point_count(radius_x: f32, radius_y: f32) -> i32 {
    let points = (((radius_x + radius_y) / 2.0) * 20.0).sqrt() as i32;
    points.max(8)
}

/// The outline of a rectangle, with rounded corners when either radius is positive. The last
/// point repeats the first.
pub fn rectangle_outline(x: f32, y: f32, width: f32, height: f32, radius_x: f32, radius_y: f32) -> Vec<Point> {
    if radius_x <= 0.0 && radius_y <= 0.0 {
        return vec![
            Point::new(x, y),
            Point::new(x + width, y),
            Point::new(x + width, y + height),
            Point::new(x, y + height),
            Point::new(x, y),
        ];
    }
    let points = ellipse_point_count(radius_x.min((width / 2.0).abs()), radius_y.min((height / 2.0).abs()));
    rounded_rectangle_outline(x, y, width, height, radius_x, radius_y, points)
}

/// LÖVE's rounded `rectangle(mode, x, y, w, h, rx, ry, points)`.
pub fn rounded_rectangle_outline(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    mut radius_x: f32,
    mut radius_y: f32,
    points: i32,
) -> Vec<Point> {
    // Radii more than half the rectangle's size aren't handled correctly.
    if width >= 0.02 {
        radius_x = radius_x.min(width / 2.0 - 0.01);
    }
    if height >= 0.02 {
        radius_y = radius_y.min(height / 2.0 - 0.01);
    }
    let points = (points / 4).max(1);
    let angle_shift = FRAC_PI_2 / (points as f32 + 1.0);
    let per_corner = (points + 2) as usize;
    let mut coordinates = vec![Point::default(); per_corner * 4 + 1];
    let mut corner = |first: usize, start_angle: f32, place: &dyn Fn(f32) -> Point| {
        let mut angle = start_angle;
        for coordinate in coordinates.iter_mut().skip(first).take(per_corner + 1) {
            *coordinate = place(angle);
            angle += angle_shift;
        }
    };
    corner(0, 0.0, &|angle| Point::new(x + radius_x * (1.0 - angle.cos()), y + radius_y * (1.0 - angle.sin())));
    corner(per_corner, FRAC_PI_2, &|angle| {
        Point::new(x + width - radius_x * (1.0 + angle.cos()), y + radius_y * (1.0 - angle.sin()))
    });
    corner(2 * per_corner, PI, &|angle| {
        Point::new(x + width - radius_x * (1.0 + angle.cos()), y + height - radius_y * (1.0 + angle.sin()))
    });
    corner(3 * per_corner, 3.0 * FRAC_PI_2, &|angle| {
        Point::new(x + radius_x * (1.0 - angle.cos()), y + height - radius_y * (1.0 + angle.sin()))
    });
    coordinates[4 * per_corner] = coordinates[0];
    coordinates
}

/// The points of an ellipse (closed: the last repeats the first).
pub fn ellipse_outline(center_x: f32, center_y: f32, radius_x: f32, radius_y: f32, points: i32) -> Vec<Point> {
    let points = points.max(1);
    let angle_shift = TAU / points as f32;
    let mut angle = 0.0f32;
    let mut coordinates = Vec::with_capacity(points as usize + 1);
    for _ in 0..points {
        coordinates.push(Point::new(center_x + radius_x * angle.cos(), center_y + radius_y * angle.sin()));
        angle += angle_shift;
    }
    coordinates.push(coordinates[0]);
    coordinates
}

/// An ellipse as triangles (fill) or a closed outline (line), as LÖVE's `ellipse`.
pub fn ellipse(
    mode: DrawMode,
    center_x: f32,
    center_y: f32,
    radius_x: f32,
    radius_y: f32,
    points: Option<i32>,
    line: LineStyle,
) -> Triangles {
    let points = points.unwrap_or_else(|| ellipse_point_count(radius_x, radius_y));
    let outline = ellipse_outline(center_x, center_y, radius_x, radius_y, points);
    match mode {
        // The centre followed by every point, the last one included.
        DrawMode::Fill => fan_from(Point::new(center_x, center_y), &outline),
        DrawMode::Line => polyline(&outline, line.half_width, line.smooth),
    }
}

/// The points of an arc, or `None` when nothing is drawn. Full circles are handed back as
/// `Err(points)` so the caller draws an ellipse instead, as LÖVE does.
#[allow(clippy::too_many_arguments)]
fn arc_coordinates(
    mode: DrawMode,
    mut arc_mode: ArcMode,
    center_x: f32,
    center_y: f32,
    radius: f32,
    angle1: f32,
    angle2: f32,
    points: i32,
) -> Option<Vec<Point>> {
    if points <= 0 || angle1 == angle2 {
        return None;
    }
    let angle_shift = (angle2 - angle1) / points as f32;
    if angle_shift == 0.0 {
        return None;
    }
    // Avoid a connecting line at sharp angles for small closed line arcs.
    if mode == DrawMode::Line && arc_mode == ArcMode::Closed && (angle1 - angle2).abs() < 4.0f32.to_radians() {
        arc_mode = ArcMode::Open;
    }
    // A filled open arc needs a closed loop.
    if mode == DrawMode::Fill && arc_mode == ArcMode::Open {
        arc_mode = ArcMode::Closed;
    }
    let mut angle = angle1;
    let mut arc_points = Vec::with_capacity(points as usize + 1);
    for _ in 0..=points {
        arc_points.push(Point::new(center_x + radius * angle.cos(), center_y + radius * angle.sin()));
        angle += angle_shift;
    }
    let center = Point::new(center_x, center_y);
    Some(match arc_mode {
        ArcMode::Pie => {
            let mut coordinates = vec![center];
            coordinates.extend(arc_points);
            coordinates.push(center);
            coordinates
        }
        ArcMode::Open => arc_points,
        ArcMode::Closed => {
            let first = arc_points[0];
            arc_points.push(first);
            arc_points
        }
    })
}

/// LÖVE's `arc`. `points` defaults to the ellipse count scaled by the arc's share of a circle.
#[allow(clippy::too_many_arguments)]
pub fn arc(
    mode: DrawMode,
    arc_mode: ArcMode,
    center_x: f32,
    center_y: f32,
    radius: f32,
    angle1: f32,
    angle2: f32,
    points: Option<i32>,
    line: LineStyle,
) -> Triangles {
    let points = points.unwrap_or_else(|| {
        let mut count = ellipse_point_count(radius, radius) as f32;
        let angle = (angle1 - angle2).abs();
        if angle < TAU {
            count *= angle / TAU;
        }
        (count + 0.5) as i32
    });
    if points > 0 && (angle1 - angle2).abs() >= TAU {
        return ellipse(mode, center_x, center_y, radius, radius, Some(points), line);
    }
    let Some(coordinates) = arc_coordinates(mode, arc_mode, center_x, center_y, radius, angle1, angle2, points) else {
        return Vec::new();
    };
    polygon(mode, &coordinates, false, line)
}

/// LÖVE's `polygon`: a triangle fan in fill mode (convex shapes only, as in LÖVE), a polyline
/// in line mode. `skip_last_filled_vertex` drops a closing duplicate from the fan.
pub fn polygon(mode: DrawMode, coordinates: &[Point], skip_last_filled_vertex: bool, line: LineStyle) -> Triangles {
    match mode {
        DrawMode::Fill => {
            let count = if skip_last_filled_vertex { coordinates.len().saturating_sub(1) } else { coordinates.len() };
            match coordinates[..count].split_first() {
                Some((first, rest)) => fan_from(*first, rest),
                None => Vec::new(),
            }
        }
        DrawMode::Line => polyline(coordinates, line.half_width, line.smooth),
    }
}

/// A rectangle (plain or rounded) as LÖVE draws it.
#[allow(clippy::too_many_arguments)]
pub fn rectangle(
    mode: DrawMode,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius_x: f32,
    radius_y: f32,
    points: Option<i32>,
    line: LineStyle,
) -> Triangles {
    if mode == DrawMode::Fill && radius_x <= 0.0 && radius_y <= 0.0 {
        let corners =
            [Point::new(x, y), Point::new(x + width, y), Point::new(x + width, y + height), Point::new(x, y + height)];
        return [corners[0], corners[1], corners[2], corners[0], corners[2], corners[3]]
            .into_iter()
            .map(|position| ShapeVertex { position, alpha: 1.0 })
            .collect();
    }
    let outline = match points {
        Some(points) if radius_x > 0.0 || radius_y > 0.0 => {
            rounded_rectangle_outline(x, y, width, height, radius_x, radius_y, points)
        }
        _ => rectangle_outline(x, y, width, height, radius_x, radius_y),
    };
    polygon(mode, &outline, true, line)
}

/// A triangle fan around `center` through `rim` (in order).
fn fan_from(center: Point, rim: &[Point]) -> Triangles {
    let mut triangles = Vec::with_capacity(rim.len().saturating_sub(1) * 3);
    for pair in rim.windows(2) {
        for position in [center, pair[0], pair[1]] {
            triangles.push(ShapeVertex { position, alpha: 1.0 });
        }
    }
    triangles
}

/// Below this, two segments count as parallel (LÖVE's `LINES_PARALLEL_EPS`).
const LINES_PARALLEL_EPSILON: f32 = 0.05;

/// One join of a miter-joined polyline (LÖVE's `MiterJoinPolyline::renderEdge`).
#[allow(clippy::too_many_arguments)]
fn miter_edge(
    anchors: &mut Vec<Point>,
    normals: &mut Vec<Point>,
    segment: &mut Point,
    segment_length: &mut f32,
    segment_normal: &mut Point,
    from: Point,
    to: Point,
    half_width: f32,
) {
    let next = to - from;
    let next_length = next.length();
    let next_normal = next.normal(half_width / next_length);
    anchors.push(from);
    anchors.push(from);
    let determinant = segment.cross(next);
    if determinant.abs() / (*segment_length * next_length) < LINES_PARALLEL_EPSILON && segment.dot(next) > 0.0 {
        normals.push(*segment_normal);
        normals.push(-*segment_normal);
    } else {
        // Cramer's rule.
        let lambda = (next_normal - *segment_normal).cross(next) / determinant;
        let miter = *segment_normal + *segment * lambda;
        normals.push(miter);
        normals.push(-miter);
    }
    *segment = next;
    *segment_normal = next_normal;
    *segment_length = next_length;
}

/// A miter-joined line through `coordinates` with LÖVE's "smooth" style: the core strip is
/// narrowed by 0.3 px and a 1 px feathered overdraw fades to transparent around it.
pub fn polyline(coordinates: &[Point], half_width: f32, smooth: bool) -> Triangles {
    if coordinates.len() < 2 {
        return Vec::new();
    }
    let pixel_size = 1.0f32;
    let half_width = if smooth { half_width - pixel_size * 0.3 } else { half_width };
    let count = coordinates.len();
    let is_looping = coordinates[0] == coordinates[count - 1];
    let mut segment =
        if is_looping { coordinates[0] - coordinates[count - 2] } else { coordinates[1] - coordinates[0] };
    let mut segment_length = segment.length();
    let mut segment_normal = segment.normal(half_width / segment_length);
    let mut anchors = Vec::with_capacity(count * 2 + 2);
    let mut normals = Vec::with_capacity(count * 2 + 2);
    let mut to = coordinates[0];
    for next in &coordinates[1..] {
        let from = to;
        to = *next;
        miter_edge(
            &mut anchors,
            &mut normals,
            &mut segment,
            &mut segment_length,
            &mut segment_normal,
            from,
            to,
            half_width,
        );
    }
    let from = to;
    let to = if is_looping { coordinates[1] } else { to + segment };
    miter_edge(
        &mut anchors,
        &mut normals,
        &mut segment,
        &mut segment_length,
        &mut segment_normal,
        from,
        to,
        half_width,
    );

    let core: Vec<Point> = anchors.iter().zip(&normals).map(|(anchor, normal)| *anchor + *normal).collect();
    let mut triangles = strip_to_triangles(&core.iter().map(|&position| (position, 1.0)).collect::<Vec<_>>());
    if smooth {
        triangles.extend(strip_to_triangles(&overdraw_strip(&core, &normals, pixel_size, is_looping)));
    }
    triangles
}

/// LÖVE's `Polyline::render_overdraw` for miter joins: an outer strip around the core whose
/// outer vertices are transparent.
fn overdraw_strip(core: &[Point], normals: &[Point], pixel_size: f32, is_looping: bool) -> Vec<(Point, f32)> {
    let vertex_count = core.len();
    let overdraw_count = 2 * vertex_count + if is_looping { 0 } else { 2 };
    let mut overdraw = vec![Point::default(); overdraw_count];
    let outward = |index: usize| core[index] + normals[index] * (pixel_size / normals[index].length());
    let mut index = 0;
    while index + 1 < vertex_count {
        overdraw[index] = core[index];
        overdraw[index + 1] = outward(index);
        index += 2;
    }
    let mut index = 0;
    while index + 1 < vertex_count {
        let mirrored = vertex_count - index - 1;
        overdraw[vertex_count + index] = core[mirrored];
        overdraw[vertex_count + index + 1] = outward(mirrored);
        index += 2;
    }
    if !is_looping && vertex_count >= 4 {
        // Push the outer vertices past the line ends so the feather covers them.
        let spacer = (overdraw[1] - overdraw[3]).normalized_to(pixel_size);
        overdraw[1] = overdraw[1] + spacer;
        overdraw[overdraw_count - 3] = overdraw[overdraw_count - 3] + spacer;
        let spacer = (overdraw[vertex_count - 1] - overdraw[vertex_count - 3]).normalized_to(pixel_size);
        overdraw[vertex_count - 1] = overdraw[vertex_count - 1] + spacer;
        overdraw[vertex_count + 1] = overdraw[vertex_count + 1] + spacer;
        // Two more triangles close the feather at the line start.
        overdraw[overdraw_count - 2] = overdraw[0];
        overdraw[overdraw_count - 1] = overdraw[1];
    }
    overdraw
        .into_iter()
        .enumerate()
        .map(|(index, position)| (position, if index % 2 == 0 { 1.0 } else { 0.0 }))
        .collect()
}

/// A triangle strip as a triangle list.
fn strip_to_triangles(strip: &[(Point, f32)]) -> Triangles {
    let mut triangles = Vec::with_capacity(strip.len().saturating_sub(2) * 3);
    for index in 0..strip.len().saturating_sub(2) {
        let corners = if index % 2 == 0 {
            [strip[index], strip[index + 1], strip[index + 2]]
        } else {
            [strip[index + 1], strip[index], strip[index + 2]]
        };
        triangles.extend(corners.map(|(position, alpha)| ShapeVertex { position, alpha }));
    }
    triangles
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: LineStyle = LineStyle { half_width: 0.5, smooth: true };

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4
    }

    #[test]
    fn ellipse_point_counts_follow_love() {
        assert_eq!(ellipse_point_count(1.0, 1.0), 8);
        assert_eq!(ellipse_point_count(8.0, 8.0), 12);
        assert_eq!(ellipse_point_count(20.0, 20.0), 20);
        assert_eq!(ellipse_point_count(100.0, 50.0), 38);
    }

    #[test]
    fn rounded_rectangle_corners() {
        // r = 8: sqrt(160) = 12 points, 3 per corner, 5 coordinates per corner + closing point.
        let outline = rectangle_outline(0.0, 0.0, 100.0, 50.0, 8.0, 8.0);
        assert_eq!(outline.len(), 21);
        assert!(close(outline[0], Point::new(0.0, 8.0)));
        assert!(close(outline[4], Point::new(8.0, 0.0)));
        assert!(close(outline[5], Point::new(92.0, 0.0)));
        assert!(close(outline[20], outline[0]));
        // A radius bigger than half the size is clamped just under it.
        let pill = rectangle_outline(0.0, 0.0, 40.0, 10.0, 20.0, 20.0);
        assert!(close(pill[0], Point::new(0.0, 4.99)));
        // The fill is a fan over the 20 points before the closing duplicate: 18 triangles.
        assert_eq!(rectangle(DrawMode::Fill, 0.0, 0.0, 100.0, 50.0, 8.0, 8.0, None, LINE).len(), 18 * 3);
        assert_eq!(rectangle(DrawMode::Fill, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0, None, LINE).len(), 6);
    }

    #[test]
    fn filled_circle_is_a_fan_through_every_point() {
        let triangles = ellipse(DrawMode::Fill, 5.0, 5.0, 8.0, 8.0, None, LINE);
        assert_eq!(triangles.len(), 12 * 3);
        assert!(close(triangles[0].position, Point::new(5.0, 5.0)));
        assert!(close(triangles[1].position, Point::new(13.0, 5.0)));
        assert!(close(triangles.last().unwrap().position, Point::new(13.0, 5.0)));
    }

    #[test]
    fn arcs() {
        // The spinner: an open line arc with 24 points.
        let spinner = arc(DrawMode::Line, ArcMode::Open, 0.0, 0.0, 10.0, 0.0, 3.0, Some(24), LineStyle::new(2.0, true));
        assert!(!spinner.is_empty());
        assert!(arc(DrawMode::Fill, ArcMode::Pie, 0.0, 0.0, 10.0, 1.0, 1.0, None, LINE).is_empty());
        // A full turn becomes a circle.
        assert_eq!(arc(DrawMode::Fill, ArcMode::Pie, 0.0, 0.0, 8.0, 0.0, TAU, None, LINE).len(), 12 * 3);
        // A pie quarter: 14 points * 1/4 -> 4 segments; fan of centre + 5 arc points + centre.
        let quarter = arc(DrawMode::Fill, ArcMode::Pie, 0.0, 0.0, 10.0, 0.0, FRAC_PI_2, None, LINE);
        assert_eq!(quarter.len(), 5 * 3);
    }

    #[test]
    fn straight_line_has_core_and_feather() {
        let line = polyline(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0)], 1.0, false);
        // Four core vertices -> two triangles; the core spans y -1..1.
        assert_eq!(line.len(), 6);
        let ys: Vec<f32> = line.iter().map(|vertex| vertex.position.y).collect();
        assert!(ys.iter().all(|y| (y.abs() - 1.0).abs() < 1e-5));
        let smooth = polyline(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0)], 1.0, true);
        assert!(smooth.len() > 6);
        assert!(smooth.iter().any(|vertex| vertex.alpha == 0.0));
        let core_extent = smooth[..6].iter().map(|vertex| vertex.position.y.abs()).fold(0.0, f32::max);
        assert!((core_extent - 0.7).abs() < 1e-5, "smooth lines are narrowed by 0.3 px");
    }

    #[test]
    fn miter_join_of_a_right_angle() {
        let line = polyline(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)], 1.0, false);
        // The outer corner of the join sits at (11, -1).
        assert!(line.iter().any(|vertex| close(vertex.position, Point::new(11.0, -1.0))));
        assert!(line.iter().any(|vertex| close(vertex.position, Point::new(9.0, 1.0))));
    }
}
