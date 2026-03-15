//! Example demonstrating safe average calculation for slices
//!
//! This example shows how to handle edge cases in mathematical functions,
//! specifically avoiding division by zero when calculating averages.

/// Calculates the average of a slice of f64 values.
///
/// # Arguments
/// * `values` - A slice of f64 values to calculate the average for
///
/// # Returns
/// * `f64::NAN` for empty slices (follows IEEE 754 standard for undefined operations)
/// * The arithmetic mean for non-empty slices
///
/// # Examples
/// ```rust
/// use rust_sample::calculate_average;
///
/// // Normal case
/// let values = [1.0, 2.0, 3.0, 4.0, 5.0];
/// assert_eq!(calculate_average(&values), 3.0);
///
/// // Empty slice case
/// let empty: [f64; 0] = [];
/// assert!(calculate_average(&empty).is_nan());
///
/// // Single value
/// let single = [42.0];
/// assert_eq!(calculate_average(&single), 42.0);
/// ```
pub fn calculate_average(values: &[f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }

    let sum: f64 = values.iter().sum();
    sum / values.len() as f64
}

/// Alternative implementation that returns Option<f64>
/// Some applications prefer explicit None over NaN for empty inputs
pub fn calculate_average_option(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }

    let sum: f64 = values.iter().sum();
    Some(sum / values.len() as f64)
}

/// Alternative implementation that returns 0.0 for empty slices
/// Some applications prefer 0.0 as a default value
pub fn calculate_average_with_default(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    let sum: f64 = values.iter().sum();
    sum / values.len() as f64
}

fn main() {
    println!("=== Average Calculation Examples ===\n");

    // Test case 1: Normal values
    let normal_values = [1.0, 2.0, 3.0, 4.0, 5.0];
    println!("Values: {:?}", normal_values);
    println!("Average: {}", calculate_average(&normal_values));
    println!();

    // Test case 2: Empty slice (the bug case)
    let empty_values: [f64; 0] = [];
    println!("Values: {:?} (empty)", empty_values);
    println!("Average: {}", calculate_average(&empty_values));
    println!("Is NaN: {}", calculate_average(&empty_values).is_nan());
    println!();

    // Test case 3: Single value
    let single_value = [42.0];
    println!("Values: {:?}", single_value);
    println!("Average: {}", calculate_average(&single_value));
    println!();

    // Test case 4: Negative values
    let negative_values = [-1.0, -2.0, -3.0];
    println!("Values: {:?}", negative_values);
    println!("Average: {}", calculate_average(&negative_values));
    println!();

    // Demonstrate alternative implementations
    println!("=== Alternative Implementations ===\n");

    println!("Option-based (empty): {:?}", calculate_average_option(&empty_values));
    println!("With default (empty): {}", calculate_average_with_default(&empty_values));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_average() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(calculate_average(&values), 3.0);
    }

    #[test]
    fn test_empty_slice_returns_nan() {
        let empty: [f64; 0] = [];
        assert!(calculate_average(&empty).is_nan());
    }

    #[test]
    fn test_single_value() {
        let single = [42.0];
        assert_eq!(calculate_average(&single), 42.0);
    }

    #[test]
    fn test_negative_values() {
        let values = [-1.0, -2.0, -3.0];
        assert_eq!(calculate_average(&values), -2.0);
    }

    #[test]
    fn test_mixed_values() {
        let values = [-2.0, 0.0, 2.0];
        assert_eq!(calculate_average(&values), 0.0);
    }

    #[test]
    fn test_floating_point_precision() {
        let values = [0.1, 0.2, 0.3];
        let result = calculate_average(&values);
        assert!((result - 0.2).abs() < f64::EPSILON);
    }

    // Tests for alternative implementations
    #[test]
    fn test_option_implementation() {
        let values = [1.0, 2.0, 3.0];
        assert_eq!(calculate_average_option(&values), Some(2.0));

        let empty: [f64; 0] = [];
        assert_eq!(calculate_average_option(&empty), None);
    }

    #[test]
    fn test_default_implementation() {
        let values = [1.0, 2.0, 3.0];
        assert_eq!(calculate_average_with_default(&values), 2.0);

        let empty: [f64; 0] = [];
        assert_eq!(calculate_average_with_default(&empty), 0.0);
    }
}