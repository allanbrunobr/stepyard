# Examples Directory

This directory contains example code demonstrating various concepts and patterns used in the minion-engine project.

## rust_sample.rs

Demonstrates safe mathematical operations in Rust, specifically:

- **Division by zero prevention**: Shows how to handle edge cases in mathematical functions
- **IEEE 754 compliance**: Uses `f64::NAN` for undefined mathematical operations
- **Alternative approaches**: Provides multiple strategies for handling empty inputs
- **Comprehensive testing**: Includes unit tests covering edge cases

### Running the Example

```bash
# Run the example
cargo run --example rust_sample

# Run the tests
cargo test --example rust_sample
```

### Bug Fix Details

**Original Issue**: The `calculate_average` function would panic with "division by zero" when called with an empty slice.

**Root Cause**: No validation for empty input before performing division.

**Solution**: Added explicit check for empty slices and return `f64::NAN` following IEEE 754 standard for undefined operations.

**Alternative Solutions**: The example also demonstrates returning `Option<f64>` or a default value (0.0) depending on application requirements.