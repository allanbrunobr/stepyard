//! Sample Rust module with intentional bugs for testing Minion Engine workflows.
//! This file has safety issues, unwrap() in production paths, and missing error handling.

use std::collections::HashMap;
use std::fs;
use std::io::Read;

/// Hardcoded credentials — security vulnerability
const DB_URL: &str = "postgres://admin:password123@localhost:5432/production";
const JWT_SECRET: &str = "super-secret-jwt-key-do-not-share";

/// BUG: unwrap() on user input — will panic in production
pub fn parse_user_id(input: &str) -> u64 {
    input.parse::<u64>().unwrap()
}

/// BUG: unnecessary clone — performance issue
pub fn process_names(names: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for name in names {
        let cloned = name.clone();
        let upper = cloned.to_uppercase();
        result.push(upper.clone());
    }
    result
}

/// BUG: unsafe block without justification
pub fn get_raw_pointer(data: &[u8]) -> *const u8 {
    unsafe {
        let ptr = data.as_ptr();
        ptr.add(0) // Pointless unsafe: same as data.as_ptr()
    }
}

/// BUG: SQL injection via format! string
pub fn build_query(table: &str, user_input: &str) -> String {
    format!("SELECT * FROM {} WHERE name = '{}'", table, user_input)
}

/// BUG: File read without proper error handling — expect() with unhelpful message
pub fn read_config(path: &str) -> String {
    fs::read_to_string(path).expect("failed")
}

/// BUG: HashMap with unbounded growth — memory leak potential
pub struct Cache {
    data: HashMap<String, Vec<u8>>,
}

impl Cache {
    pub fn new() -> Self {
        Cache {
            data: HashMap::new(),
        }
    }

    /// BUG: Never evicts entries — unbounded memory growth
    pub fn insert(&mut self, key: String, value: Vec<u8>) {
        self.data.insert(key, value);
    }

    /// BUG: unwrap on get — will panic on missing key
    pub fn get(&self, key: &str) -> &Vec<u8> {
        self.data.get(key).unwrap()
    }
}

/// BUG: Password comparison susceptible to timing attacks
pub fn verify_password(stored: &str, input: &str) -> bool {
    stored == input
}

/// BUG: Division by zero not handled
pub fn calculate_average(values: &[f64]) -> f64 {
    let sum: f64 = values.iter().sum();
    sum / values.len() as f64
}

/// BUG: Integer overflow possible in release mode
pub fn factorial(n: u32) -> u32 {
    let mut result: u32 = 1;
    for i in 2..=n {
        result *= i; // Will overflow for n > 12
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_id() {
        assert_eq!(parse_user_id("42"), 42);
    }

    // BUG: This test should exist but doesn't:
    // #[test]
    // fn test_parse_invalid_id_returns_error() {
    //     // parse_user_id("abc") should not panic
    // }

    #[test]
    fn test_process_names() {
        let names = vec!["alice".to_string(), "bob".to_string()];
        let result = process_names(&names);
        assert_eq!(result, vec!["ALICE", "BOB"]);
    }

    #[test]
    fn test_calculate_average() {
        assert_eq!(calculate_average(&[10.0, 20.0, 30.0]), 20.0);
    }

    // BUG: Missing test for empty slice (division by zero)
}
