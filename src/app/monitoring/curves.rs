//! Presentation geometry for telemetry, independent of collection and rate math.
//! Cubic curves pass through every measurement. Limited slopes preserve peaks
//! and plateaus without overshooting adjacent samples or inventing new readings.
use eframe::egui::{Color32, Mesh, Pos2, pos2};

/// Smooth one continuous, increasing-time segment in plot coordinates. Callers
/// split missing readings and pauses before interpolation; two points stay linear.
/// Subdivision follows screen width, with a bounded amount of work per interval.
pub(super) fn smooth(points: &[Pos2]) -> Vec<Pos2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let slopes: Vec<_> = points
        .windows(2)
        .map(|p| {
            let width = p[1].x - p[0].x;
            if width > 0. {
                (p[1].y - p[0].y) / width
            } else {
                0.
            }
        })
        .collect();
    let mut tangents = vec![slopes[0]];
    for pair in slopes.windows(2) {
        // At a reversal or plateau the tangent is horizontal. Else limit its
        // magnitude to both neighbouring secants, including irregular intervals.
        tangents.push(if pair[0] * pair[1] > 0. {
            pair[0].signum() * pair[0].abs().min(pair[1].abs())
        } else {
            0.
        });
    }
    tangents.push(*slopes.last().unwrap());
    let mut result = vec![points[0]];
    for (i, pair) in points.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        let width = b.x - a.x;
        let steps = ((width / 4.).ceil() as usize).clamp(2, 32);
        for step in 1..steps {
            let t = step as f32 / steps as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            let value = (2. * t3 - 3. * t2 + 1.) * a.y
                + (t3 - 2. * t2 + t) * width * tangents[i]
                + (-2. * t3 + 3. * t2) * b.y
                + (t3 - t2) * width * tangents[i + 1];
            result.push(pos2(
                a.x + width * t,
                value.clamp(a.y.min(b.y), a.y.max(b.y)),
            ));
        }
        // Keep original samples exact, including extrema and flat intervals.
        result.push(b);
    }
    result
}

/// A triangle strip under a curve, fading towards its baseline. Fill and stroke
/// share the same geometry; separate segments never acquire connecting triangles.
pub(super) fn area(points: &[Pos2], baseline: f32, color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    for point in points {
        mesh.colored_vertex(*point, color.gamma_multiply(0.22));
        mesh.colored_vertex(pos2(point.x, baseline), color.gamma_multiply(0.02));
    }
    for i in 0..points.len().saturating_sub(1) as u32 {
        let top = i * 2;
        mesh.add_triangle(top, top + 1, top + 2);
        mesh.add_triangle(top + 2, top + 1, top + 3);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irregular_intervals_preserve_measurements_extrema_and_plateaus() {
        let samples = [
            pos2(0., 80.),
            pos2(17., 0.),
            pos2(25., 0.),
            pos2(171., 100.),
            pos2(176., 30.),
            pos2(260., 55.),
        ];
        let curve = smooth(&samples);
        for point in samples {
            assert!(curve.contains(&point), "Lost a measured value: {point:?}");
        }
        assert!(curve.windows(2).all(|p| p[0].x < p[1].x));
        for interval in samples.windows(2) {
            let points: Vec<_> = curve
                .iter()
                .filter(|p| p.x >= interval[0].x && p.x <= interval[1].x)
                .collect();
            assert!(points.len() > 2);
            assert!(points.iter().all(|p| {
                p.y.is_finite()
                    && p.y >= interval[0].y.min(interval[1].y)
                    && p.y <= interval[0].y.max(interval[1].y)
            }));
            let rising = interval[1].y >= interval[0].y;
            assert!(points.windows(2).all(|p| if rising {
                p[1].y >= p[0].y
            } else {
                p[1].y <= p[0].y
            }));
        }
        assert!(curve.len() <= 32 * (samples.len() - 1) + 1);
    }

    #[test]
    fn small_and_flat_segments_do_not_acquire_artificial_activity() {
        assert!(smooth(&[]).is_empty());
        let points = [pos2(0., 50.), pos2(200., 50.), pos2(400., 50.)];
        assert_eq!(smooth(&points[..1]), points[..1]);
        assert_eq!(smooth(&points[..2]), points[..2]);
        assert!(smooth(&points).iter().all(|p| p.y == 50.));
        let collapsed = [pos2(0., 50.), pos2(0., 0.), pos2(1., 80.)];
        assert!(
            smooth(&collapsed)
                .iter()
                .all(|p| p.x.is_finite() && p.y.is_finite())
        );
    }

    #[test]
    fn gradient_fill_tracks_the_curve_with_valid_triangles_and_transparent_baseline() {
        let points = smooth(&[pos2(0., 60.), pos2(50., 10.), pos2(100., 40.)]);
        let mesh = area(&points, 100., Color32::LIGHT_BLUE);
        assert!(mesh.is_valid());
        assert_eq!(mesh.indices.len(), (points.len() - 1) * 6);
        for (pair, point) in mesh.vertices.chunks_exact(2).zip(points) {
            assert_eq!(pair[0].pos, point);
            assert_eq!(pair[1].pos, pos2(point.x, 100.));
            assert!(pair[0].color.a() > pair[1].color.a());
        }
    }
}
