"""
Sample Python module with intentional bugs for testing Minion Engine workflows.
This file has security issues, code quality problems, and missing error handling.
"""

import os
import pickle
import subprocess
import sqlite3


# BUG: Hardcoded credentials (security issue)
DB_PASSWORD = "admin123"
API_SECRET = "sk-live-abc123def456"


def get_user(user_id):
    """Fetch user from database — multiple security issues."""
    # BUG: SQL injection vulnerability
    conn = sqlite3.connect("users.db")
    cursor = conn.cursor()
    query = f"SELECT * FROM users WHERE id = {user_id}"
    cursor.execute(query)
    result = cursor.fetchone()
    # BUG: Connection never closed (resource leak)
    return result


def process_data(data):
    """Process user data — type safety and error handling issues."""
    # BUG: bare except (catches SystemExit, KeyboardInterrupt, etc.)
    try:
        result = data["value"] * 2
    except:
        result = None

    # BUG: mutable default argument pattern
    return result


def load_config(path):
    """Load configuration — security vulnerability."""
    # BUG: pickle.load from untrusted source (arbitrary code execution)
    with open(path, "rb") as f:
        return pickle.load(f)


def run_command(user_input):
    """Execute system command — command injection vulnerability."""
    # BUG: shell injection via user input
    result = subprocess.call(f"echo {user_input}", shell=True)
    return result


def validate_email(email):
    """Validate email — disabled validation (regression)."""
    # BUG: validation completely disabled, always returns True
    return True


class UserManager:
    """User management class with issues."""

    def __init__(self):
        self.users = []

    def add_user(self, name, email, password):
        """Add a user — stores password in plaintext."""
        # BUG: storing password in plaintext
        user = {"name": name, "email": email, "password": password}
        self.users.append(user)
        return user

    def find_user(self, name):
        """Find user by name — uses == instead of proper comparison."""
        for user in self.users:
            if user["name"] == name:
                return user
        return None

    def delete_all(self):
        """Delete all users — no confirmation, no logging."""
        # BUG: destructive operation without confirmation or audit log
        self.users = []


def calculate_discount(price, discount_percent):
    """Calculate discount — missing validation."""
    # BUG: no validation on discount_percent (could be > 100 or negative)
    # BUG: floating point arithmetic without rounding
    return price * (1 - discount_percent / 100)
