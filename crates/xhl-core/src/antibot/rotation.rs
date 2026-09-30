use std::f64::consts::PI;

/// Convert rotation from degrees to a 2D rotation matrix
pub fn convert_rotation_to_matrix(degrees: f64) -> Vec<f64> {
    let radians = degrees * PI / 180.0;
    let cos = radians.cos();
    let sin = radians.sin();
    vec![cos, -sin, sin, cos]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rotation_matrix() {
        let matrix = convert_rotation_to_matrix(90.0);
        assert!((matrix[0] - 0.0).abs() < 0.00001);
        assert!((matrix[1] - (-1.0)).abs() < 0.00001);
        assert!((matrix[2] - 1.0).abs() < 0.00001);
        assert!((matrix[3] - 0.0).abs() < 0.00001);
    }
}
