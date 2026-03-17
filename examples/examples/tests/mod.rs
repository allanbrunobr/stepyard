/// Additional comprehensive tests for the calculate_average function
///
/// This module provides extensive testing coverage beyond the unit tests
/// in rust_sample.rs to ensure the bug fix is robust and handles all edge cases.

#[cfg(test)]
mod comprehensive_tests {
    use super::super::rust_sample::calculate_average;
    use std::f64::consts::{E, PI};

    // === Basic Functionality Tests ===

    #[test]
    fn test_integer_values_as_floats() {
        let numbers = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(calculate_average(&numbers), 2.5);
    }

    #[test]
    fn test_all_same_values() {
        let numbers = [5.0, 5.0, 5.0, 5.0, 5.0];
        assert_eq!(calculate_average(&numbers), 5.0);
    }

    #[test]
    fn test_alternating_positive_negative() {
        let numbers = [1.0, -1.0, 2.0, -2.0, 3.0, -3.0];
        assert_eq!(calculate_average(&numbers), 0.0);
    }

    // === Edge Cases Tests ===

    #[test]
    fn test_empty_slice_returns_nan() {
        let empty: [f64; 0] = [];
        let result = calculate_average(&empty);
        assert!(result.is_nan());
        assert!(!result.is_infinite());
        assert!(!result.is_finite());
    }

    #[test]
    fn test_single_zero() {
        let single = [0.0];
        assert_eq!(calculate_average(&single), 0.0);
    }

    #[test]
    fn test_single_negative() {
        let single = [-42.0];
        assert_eq!(calculate_average(&single), -42.0);
    }

    #[test]
    fn test_two_elements() {
        let pair = [10.0, 20.0];
        assert_eq!(calculate_average(&pair), 15.0);
    }

    // === Precision and Numerical Stability Tests ===

    #[test]
    fn test_high_precision_decimals() {
        let numbers = [0.1, 0.2, 0.3];
        let result = calculate_average(&numbers);
        let expected = 0.2;
        assert!((result - expected).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn test_very_large_numbers() {
        let numbers = [1e100, 2e100, 3e100];
        let result = calculate_average(&numbers);
        assert!((result - 2e100).abs() < 1e95);
    }

    #[test]
    fn test_very_small_positive_numbers() {
        let numbers = [1e-100, 2e-100, 3e-100];
        let result = calculate_average(&numbers);
        assert!(result > 0.0);
        assert!((result - 2e-100).abs() < 1e-105);
    }

    #[test]
    fn test_mixed_magnitude_numbers() {
        let numbers = [1e-10, 1.0, 1e10];
        let result = calculate_average(&numbers);
        // The large number dominates, so result should be close to 1e10/3
        assert!(result > 1e9);
        assert!(result < 1e11);
    }

    // === Special Float Values Tests ===

    #[test]
    fn test_positive_infinity() {
        let numbers = [1.0, 2.0, f64::INFINITY];
        let result = calculate_average(&numbers);
        assert!(result.is_infinite());
        assert!(result.is_sign_positive());
    }

    #[test]
    fn test_negative_infinity() {
        let numbers = [1.0, 2.0, f64::NEG_INFINITY];
        let result = calculate_average(&numbers);
        assert!(result.is_infinite());
        assert!(result.is_sign_negative());
    }

    #[test]
    fn test_both_infinities() {
        let numbers = [f64::INFINITY, f64::NEG_INFINITY];
        let result = calculate_average(&numbers);
        assert!(result.is_nan()); // inf + (-inf) = NaN
    }

    #[test]
    fn test_nan_input() {
        let numbers = [1.0, f64::NAN, 3.0];
        let result = calculate_average(&numbers);
        assert!(result.is_nan());
    }

    #[test]
    fn test_multiple_nans() {
        let numbers = [f64::NAN, f64::NAN, f64::NAN];
        let result = calculate_average(&numbers);
        assert!(result.is_nan());
    }

    #[test]
    fn test_subnormal_numbers() {
        let numbers = [f64::MIN_POSITIVE, f64::MIN_POSITIVE * 2.0];
        let result = calculate_average(&numbers);
        assert!(result > 0.0);
        assert!(result.is_finite());
    }

    // === Mathematical Constants Tests ===

    #[test]
    fn test_mathematical_constants() {
        let numbers = [PI, E, 1.0, 0.0];
        let result = calculate_average(&numbers);
        let expected = (PI + E + 1.0 + 0.0) / 4.0;
        assert!((result - expected).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn test_zero_and_negative_zero() {
        let numbers = [0.0, -0.0];
        let result = calculate_average(&numbers);
        assert_eq!(result, 0.0);
        // Both 0.0 and -0.0 should average to 0.0
    }

    // === Stress Tests ===

    #[test]
    fn test_large_array() {
        let numbers: Vec<f64> = (1..=1000).map(|x| x as f64).collect();
        let result = calculate_average(&numbers);
        let expected = 500.5; // Average of 1 to 1000
        assert!((result - expected).abs() < f64::EPSILON * 1000.0);
    }

    #[test]
    fn test_alternating_large_array() {
        let numbers: Vec<f64> = (0..1000).map(|x| if x % 2 == 0 { 1.0 } else { -1.0 }).collect();
        let result = calculate_average(&numbers);
        assert!((result - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_powers_of_two() {
        let numbers: Vec<f64> = (0..10).map(|x| 2.0_f64.powi(x)).collect();
        let result = calculate_average(&numbers);
        assert!(result > 0.0);
        assert!(result < 1024.0); // Should be less than 2^10
    }

    // === Regression Tests (Specific to the Bug Fix) ===

    #[test]
    fn test_regression_empty_slice_no_panic() {
        // This is the core regression test for the original bug
        let empty: Vec<f64> = vec![];
        let result = calculate_average(&empty);
        assert!(result.is_nan());
        // If we reach this point, no panic occurred
    }

    #[test]
    fn test_regression_multiple_empty_calls() {
        // Ensure multiple calls to empty slices work correctly
        let empty: [f64; 0] = [];
        for _ in 0..10 {
            let result = calculate_average(&empty);
            assert!(result.is_nan());
        }
    }

    #[test]
    fn test_regression_empty_vs_nonempty() {
        let empty: [f64; 0] = [];
        let nonempty = [5.0];

        let empty_result = calculate_average(&empty);
        let nonempty_result = calculate_average(&nonempty);

        assert!(empty_result.is_nan());
        assert_eq!(nonempty_result, 5.0);
    }
}

// === Integration Tests ===

#[cfg(test)]
mod integration_tests {
    use super::super::rust_sample::calculate_average;

    #[test]
    fn test_real_world_temperature_data() {
        // Simulate real temperature readings in Celsius
        let temperatures = [23.5, 24.1, 22.8, 25.3, 24.9, 23.2, 22.1];
        let avg_temp = calculate_average(&temperatures);
        assert!(avg_temp > 22.0 && avg_temp < 26.0);

        // Test with empty temperature data (sensor malfunction)
        let no_readings: [f64; 0] = [];
        let result = calculate_average(&no_readings);
        assert!(result.is_nan()); // Should handle gracefully
    }

    #[test]
    fn test_financial_data() {
        // Stock prices over a week
        let prices = [150.25, 152.10, 149.80, 151.45, 153.20];
        let avg_price = calculate_average(&prices);
        assert!(avg_price > 149.0 && avg_price < 154.0);

        // Empty trading data (market closed)
        let no_trades: Vec<f64> = vec![];
        let result = calculate_average(&no_trades);
        assert!(result.is_nan());
    }

    #[test]
    fn test_sensor_readings_with_outliers() {
        // Sensor readings with one faulty reading
        let readings = [1.1, 1.2, 1.0, 1000.0, 1.3]; // 1000.0 is clearly an outlier
        let avg = calculate_average(&readings);
        // Average will be skewed by outlier, but function should still work
        assert!(avg > 200.0); // Outlier dominates
        assert!(avg.is_finite());
    }
}