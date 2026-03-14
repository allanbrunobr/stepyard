"""Simple Flask API for user management."""

from flask import Flask, jsonify, request
import requests

app = Flask(__name__)

# In-memory user store
users_db: dict[int, dict] = {}
next_id: int = 1


def validate_email(email: str) -> bool:
    """Validate email format."""
    return "@" in email and "." in email.split("@")[1]


def validate_user_data(data: dict) -> tuple[bool, str]:
    """Validate user creation payload."""
    if not data.get("name"):
        return False, "Name is required"
    if not data.get("email"):
        return False, "Email is required"
    if not validate_email(data["email"]):
        return False, "Invalid email format"
    return True, ""


@app.route("/users", methods=["GET"])
def list_users():
    """List all users."""
    return jsonify(list(users_db.values()))


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
    valid, error = validate_user_data(data)
    if not valid:
        return jsonify({"error": error}), 400

    user = {
        "id": next_id,
        "name": data["name"],
        "email": data["email"],
        "role": data.get("role", "user"),
    }
    users_db[next_id] = user
    next_id += 1
    return jsonify(user), 201


@app.route("/users/<int:user_id>", methods=["DELETE"])
def delete_user(user_id: int):
    """Delete a user."""
    if user_id not in users_db:
        return jsonify({"error": "User not found"}), 404
    del users_db[user_id]
    return "", 204


def fetch_external_profile(email: str) -> dict | None:
    """Fetch user profile from external service."""
    try:
        resp = requests.get(f"https://api.example.com/profiles/{email}", timeout=5)
        if resp.status_code == 200:
            return resp.json()
    except requests.RequestException:
        pass
    return None


if __name__ == "__main__":
    app.run(debug=True, port=5000)
