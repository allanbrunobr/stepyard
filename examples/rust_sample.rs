//! Rust sample code demonstrating mathematical calculations
//!
//! This module contains examples of numerical operations with proper
//! error handling for edge cases.

/// Calculates the average of a slice of f64 numbers.
///
/// # Arguments
/// * `numbers` - A slice of f64 values
///
/// # Returns
/// * For empty slices: returns `f64::NAN`
/// * For non-empty slices: returns the arithmetic mean
///
/// # Examples
/// ```
/// use rust_sample::calculate_average;
///
/// assert_eq!(calculate_average(&[1.0, 2.0, 3.0]), 2.0);
/// assert!(calculate_average(&[]).is_nan());
/// ```
pub fn calculate_average(numbers: &[f64]) -> f64 {
    if numbers.is_empty() {
        return f64::NAN;
    }

    let sum: f64 = numbers.iter().sum();
    sum / numbers.len() as f64
}

/// Alternative implementation that returns Option<f64>
///
/// # Arguments
/// * `numbers` - A slice of f64 values
///
/// # Returns
/// * `None` for empty slices
/// * `Some(average)` for non-empty slices
pub fn calculate_average_option(numbers: &[f64]) -> Option<f64> {
    if numbers.is_empty() {
        return None;
    }

    let sum: f64 = numbers.iter().sum();
    Some(sum / numbers.len() as f64)
}

/// Alternative implementation that returns Result<f64, &'static str>
///
/// # Arguments
/// * `numbers` - A slice of f64 values
///
/// # Returns
/// * `Err("Empty slice")` for empty slices
/// * `Ok(average)` for non-empty slices
pub fn calculate_average_result(numbers: &[f64]) -> Result<f64, &'static str> {
    if numbers.is_empty() {
        return Err("Cannot calculate average of empty slice");
    }

    let sum: f64 = numbers.iter().sum();
    Ok(sum / numbers.len() as f64)
}

fn main() {
    println!("=== Rust Sample: calculate_average examples ===\n");

    // Test cases
    let test_cases = vec![
        vec![],
        vec![5.0],
        vec![1.0, 2.0, 3.0, 4.0, 5.0],
        vec![-1.0, 0.0, 1.0],
        vec![f64::INFINITY, 1.0, 2.0],
    ];

    for (i, case) in test_cases.iter().enumerate() {
        println!("Test case {}: {:?}", i + 1, case);

        let avg = calculate_average(case);
        println!("  calculate_average: {}", avg);

        let avg_opt = calculate_average_option(case);
        println!("  calculate_average_option: {:?}", avg_opt);

        let avg_result = calculate_average_result(case);
        println!("  calculate_average_result: {:?}", avg_result);

        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_slice_returns_nan() {
        let result = calculate_average(&[]);
        assert!(result.is_nan());
    }

    #[test]
    fn test_single_element() {
        let result = calculate_average(&[42.0]);
        assert_eq!(result, 42.0);
    }

    #[test]
    fn test_multiple_elements() {
        let result = calculate_average(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(result, 3.0);
    }

    #[test]
    fn test_negative_numbers() {
        let result = calculate_average(&[-2.0, -1.0, 0.0, 1.0, 2.0]);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_option_variant_empty() {
        let result = calculate_average_option(&[]);
        assert_eq!(result, None);
    }

    #[test]
    fn test_option_variant_non_empty() {
        let result = calculate_average_option(&[2.0, 4.0, 6.0]);
        assert_eq!(result, Some(4.0));
    }

    #[test]
    fn test_result_variant_empty() {
        let result = calculate_average_result(&[]);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Cannot calculate average of empty slice"
        );
    }

    #[test]
    fn test_result_variant_non_empty() {
        let result = calculate_average_result(&[10.0, 20.0, 30.0]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 20.0);
    }
}
