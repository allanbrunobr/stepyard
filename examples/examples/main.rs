mod rust_sample;
use rust_sample::calculate_average;

fn main() {
    println!("=== Calculate Average Bug Fix Demo ===\n");

    // Normal case
    let numbers = [1.0, 2.0, 3.0, 4.0, 5.0];
    println!("📊 Average of {:?}: {}", numbers, calculate_average(&numbers));

    // Edge case: empty slice (this used to panic)
    let empty: [f64; 0] = [];
    let result = calculate_average(&empty);
    println!("🔧 Average of empty slice: {} (is_nan: {})", result, result.is_nan());
    println!("   ✅ No panic! Previously this would crash the program.");

    // Single element
    let single = [42.5];
    println!("🔸 Average of {:?}: {}", single, calculate_average(&single));

    // Negative numbers
    let negative = [-1.0, -2.0, -3.0];
    println!("➖ Average of {:?}: {}", negative, calculate_average(&negative));

    // Mixed positive and negative
    let mixed = [-2.0, 0.0, 2.0];
    println!("🔀 Average of {:?}: {}", mixed, calculate_average(&mixed));

    // Decimal precision
    let decimal = [1.1, 2.2, 3.3, 4.4, 5.5];
    println!("🔢 Average of {:?}: {:.2}", decimal, calculate_average(&decimal));

    // Large numbers
    let large = [1e10, 2e10, 3e10];
    println!("📈 Average of large numbers: {:.2e}", calculate_average(&large));

    // Very small numbers
    let small = [1e-10, 2e-10, 3e-10];
    println!("🔬 Average of small numbers: {:.2e}", calculate_average(&small));

    // Edge case: single very large number
    let huge = [f64::MAX / 2.0];
    println!("🌌 Average of [MAX/2]: {:.2e}", calculate_average(&huge));

    // Demonstrating NaN handling with infinity
    let with_inf = [1.0, f64::INFINITY, 3.0];
    let inf_result = calculate_average(&with_inf);
    println!("♾️  Average with infinity: {} (is_infinite: {})", inf_result, inf_result.is_infinite());

    // Demonstrating NaN propagation
    let with_nan = [1.0, f64::NAN, 3.0];
    let nan_result = calculate_average(&with_nan);
    println!("❓ Average with NaN: {} (is_nan: {})", nan_result, nan_result.is_nan());

    println!("\n✅ All cases handled without panicking!");
    println!("🛠️  Bug fix complete: Empty slices now return NaN instead of crashing");

    // Demonstrate what the fix prevents
    println!("\n🚨 Original Bug Demonstration:");
    println!("   Before fix: calculate_average(&[]) would panic with 'attempt to divide by zero'");
    println!("   After fix:  calculate_average(&[]) returns NaN ({})", result);
    println!("   This allows graceful error handling instead of program crashes");
}