/// Example demonstrating the calculate_average function bug fix
///
/// This module contains the calculate_average function that originally
/// had a division by zero bug when given empty slices.

/// Calculates the average of a slice of f64 numbers.
///
/// # Arguments
/// * `numbers` - A slice of f64 values
///
/// # Returns
/// * The arithmetic mean of the numbers
/// * `f64::NAN` if the slice is empty (after fix)
///
/// # Examples
/// ```
/// let nums = [1.0, 2.0, 3.0];
/// assert_eq!(calculate_average(&nums), 2.0);
///
/// let empty: [f64; 0] = [];
/// assert!(calculate_average(&empty).is_nan());
/// ```
///
/// # Panics
/// Originally this function would panic on empty slices due to division by zero.
/// This has been fixed to return f64::NAN instead.
pub fn calculate_average(numbers: &[f64]) -> f64 {
    // FIXED VERSION: Check for empty slice first
    if numbers.is_empty() {
        return f64::NAN;
    }

    let sum: f64 = numbers.iter().sum();
    sum / numbers.len() as f64
}

/// Demonstrates the original buggy behavior for reference
/// This function will panic if given an empty slice
#[allow(dead_code)]
fn calculate_average_buggy_original(numbers: &[f64]) -> f64 {
    let sum: f64 = numbers.iter().sum();
    sum / numbers.len() as f64  // This causes division by zero panic
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_average_normal_case() {
        let numbers = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(calculate_average(&numbers), 3.0);
    }

    #[test]
    fn test_calculate_average_empty_slice() {
        let empty: [f64; 0] = [];
        assert!(calculate_average(&empty).is_nan());
    }

    #[test]
    fn test_calculate_average_single_element() {
        let single = [42.5];
        assert_eq!(calculate_average(&single), 42.5);
    }

    #[test]
    fn test_calculate_average_negative_numbers() {
        let numbers = [-1.0, -2.0, -3.0];
        assert_eq!(calculate_average(&numbers), -2.0);
    }

    #[test]
    fn test_calculate_average_mixed_numbers() {
        let numbers = [-2.0, 0.0, 2.0];
        assert_eq!(calculate_average(&numbers), 0.0);
    }

    #[test]
    fn test_calculate_average_large_numbers() {
        let numbers = [f64::MAX / 2.0, f64::MAX / 2.0];
        let result = calculate_average(&numbers);
        assert!((result - f64::MAX / 2.0).abs() < f64::EPSILON * f64::MAX);
    }

    #[test]
    fn test_calculate_average_decimal_precision() {
        let numbers = [1.1, 2.2, 3.3];
        let expected = 2.2;
        let result = calculate_average(&numbers);
        assert!((result - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_average_very_small_numbers() {
        let numbers = [f64::MIN_POSITIVE, f64::MIN_POSITIVE * 2.0, f64::MIN_POSITIVE * 3.0];
        let result = calculate_average(&numbers);
        assert!(result > 0.0);
        assert!((result - f64::MIN_POSITIVE * 2.0).abs() < f64::MIN_POSITIVE);
    }

    #[test]
    fn test_calculate_average_infinity_handling() {
        let numbers = [1.0, 2.0, f64::INFINITY];
        let result = calculate_average(&numbers);
        assert!(result.is_infinite());
    }

    #[test]
    fn test_calculate_average_with_nan_input() {
        let numbers = [1.0, f64::NAN, 3.0];
        let result = calculate_average(&numbers);
        assert!(result.is_nan());
    }

    #[test]
    #[should_panic]
    fn test_buggy_version_panics_on_empty() {
        let empty: [f64; 0] = [];
        calculate_average_buggy_original(&empty);
    }
}