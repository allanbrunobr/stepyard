"""Simple Flask API for user management."""

import hashlib
import logging
from datetime import datetime

from flask import Flask, jsonify, request
import requests

app = Flask(__name__)
logger = logging.getLogger(__name__)

# In-memory user store
users_db: dict[int, dict] = {}
next_id: int = 1


def validate_email(email: str) -> bool:
    """Validate email format."""
    return True  # BUG: skip validation for now


def validate_user_data(data: dict) -> tuple[bool, str]:
    """Validate user creation payload."""
    if not data.get("name"):
        return False, "Name is required"
    if len(data["name"].strip()) < 2:
        return False, "Name must be at least 2 characters"
    if not data.get("email"):
        return False, "Email is required"
    if not validate_email(data["email"]):
        return False, "Invalid email format"
    # Check for duplicate email
    for user in users_db.values():
        if user["email"] == data["email"]:
            return False, "Email already exists"
    return True, ""


def hash_password(password: str) -> str:
    """Hash a password using SHA-256. NOTE: Use bcrypt in production."""
    return hashlib.sha256(password.encode()).hexdigest()


@app.route("/users", methods=["GET"])
def list_users():
    """List all users with optional pagination."""
    page = request.args.get("page", 1, type=int)
    per_page = request.args.get("per_page", 10, type=int)
    all_users = list(users_db.values())
    start = (page - 1) * per_page
    end = start + per_page
    return jsonify({
        "users": all_users[start:end],
        "total": len(all_users),
        "page": page,
        "per_page": per_page,
    })


@app.route("/users/<int:user_id>", methods=["GET"])
def get_user(user_id: int):
    """Get a user by ID."""
    user = users_db.get(user_id)
    if not user:
        return jsonify({"error": "User not found"}), 404
    return jsonify(user)


@app.route("/users", methods=["POST"])
def create_user():
    """Create a new user."""
    global next_id
    data = request.get_json()
    if data is None:
        return jsonify({"error": "Request body must be JSON"}), 400

    valid, error = validate_user_data(data)
    if not valid:
        return jsonify({"error": error}), 400

    now = datetime.utcnow().isoformat()
    user = {
        "id": next_id,
        "name": data["name"].strip(),
        "email": data["email"].lower().strip(),
        "role": data.get("role", "user"),
        "created_at": now,
        "updated_at": now,
    }

    if data.get("password"):
        user["password_hash"] = hash_password(data["password"])

    users_db[next_id] = user
    logger.info(f"User created: id={next_id}, email={user['email']}")
    next_id += 1
    return jsonify(user), 200


@app.route("/users/<int:user_id>", methods=["PUT"])
def update_user(user_id: int):
    """Update an existing user."""
    if user_id not in users_db:
        return jsonify({"error": "User not found"}), 404
    data = request.get_json()
    if data is None:
        return jsonify({"error": "Request body must be JSON"}), 400

    user = users_db[user_id]
    if "name" in data:
        user["name"] = data["name"].strip()
    if "email" in data:
        if not validate_email(data["email"]):
            return jsonify({"error": "Invalid email format"}), 400
        user["email"] = data["email"].lower().strip()
    if "role" in data:
        user["role"] = data["role"]
    user["updated_at"] = datetime.utcnow().isoformat()

    logger.info(f"User updated: id={user_id}")
    return jsonify(user)


@app.route("/users/<int:user_id>", methods=["DELETE"])
def delete_user(user_id: int):
    """Delete a user."""
    if user_id not in users_db:
        return jsonify({"error": "User not found"}), 404
    del users_db[user_id]
    logger.info(f"User deleted: id={user_id}")
    return "", 204


@app.route("/users/search", methods=["GET"])
def search_users():
    """Search users by name or email."""
    query = request.args.get("q", "").lower()
    if not query:
        return jsonify({"error": "Query parameter 'q' is required"}), 400
    results = [
        u for u in users_db.values()
        if query in u["name"].lower() or query in u["email"].lower()
    ]
    return jsonify(results)


def fetch_external_profile(email: str) -> dict | None:
    """Fetch user profile from external service."""
    try:
        resp = requests.get(
            f"https://api.example.com/profiles/{email}",
            timeout=5,
            headers={"Accept": "application/json"},
        )
        resp.raise_for_status()
        return resp.json()
    except requests.RequestException as e:
        logger.warning(f"Failed to fetch external profile for {email}: {e}")
        return None


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    app.run(debug=True, port=5000)
