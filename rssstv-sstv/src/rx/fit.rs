//! The least-squares line every sync fit is drawn with.
//!
//! Acquisition and slant estimation both fit sync centers against a line;
//! sharing the arithmetic keeps the two from drifting apart. What each fit
//! accepts as input, and what residual it tolerates, stays its own.

/// Fits `y = intercept + slope * x` by ordinary least squares.
///
/// Returns `(slope, intercept, mean_squared_residual)`, or nothing when the
/// points share one `x` and no line is determined.
pub(super) fn least_squares_line(points: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    if points.len() < 2 {
        return None;
    }
    let count = points.len() as f64;
    let mean_x = points.iter().map(|(x, _)| x).sum::<f64>() / count;
    let mean_y = points.iter().map(|(_, y)| y).sum::<f64>() / count;
    let denominator = points
        .iter()
        .map(|(x, _)| (x - mean_x) * (x - mean_x))
        .sum::<f64>();
    if denominator <= 0.0 {
        return None;
    }
    let slope = points
        .iter()
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum::<f64>()
        / denominator;
    let intercept = mean_y - slope * mean_x;
    let mean_squared_residual = points
        .iter()
        .map(|(x, y)| {
            let residual = y - (intercept + slope * x);
            residual * residual
        })
        .sum::<f64>()
        / count;
    Some((slope, intercept, mean_squared_residual))
}
